#!/usr/bin/env python3
"""Loopback MySQL STARTTLS relay with a private CA and an exact leaf pin.

No database password is configured here. Both sides must negotiate TLS; the
upstream chain and leaf are authenticated before accepting downstream TLS or
forwarding any authentication payload. Intended for legacy certificates without
SANs. The CA and leaf pin must come from an independently trusted source.
"""
import argparse
import hashlib
import hmac
import json
import re
import select
import signal
import socket
import ssl
import sys
import threading
from pathlib import Path


def packet(sock):
    def exact(count):
        result = bytearray()
        while len(result) < count:
            chunk = sock.recv(count - len(result))
            if not chunk:
                raise ConnectionError('incomplete handshake')
            result.extend(chunk)
        return bytes(result)
    header = exact(4)
    size = int.from_bytes(header[:3], 'little')
    if not 0 < size <= 8192:
        raise ValueError('invalid handshake size')
    return header + exact(size)


class Relay:
    def __init__(self, config):
        self.config = config
        pin = config['upstream_leaf_sha256'].lower()
        if not re.fullmatch(r'[0-9a-f]{64}', pin):
            raise ValueError('exact SHA-256 leaf pin required')
        self.pin = pin
        # Explicit CA only: never add system roots to this instance's trust.
        self.upstream_tls = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
        self.upstream_tls.check_hostname = False  # Identity is the exact leaf pin below.
        self.upstream_tls.verify_mode = ssl.CERT_REQUIRED
        self.upstream_tls.minimum_version = ssl.TLSVersion.TLSv1_2
        self.upstream_tls.load_verify_locations(cafile=config['upstream_ca'])
        self.local_tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        self.local_tls.minimum_version = ssl.TLSVersion.TLSv1_2
        self.local_tls.load_cert_chain(config['local_cert'], config['local_key'])
        self.slots = threading.BoundedSemaphore(16)
        self.stop = threading.Event()
        self.listener = None

    def connection(self, client):
        upstream = None
        try:
            client.settimeout(10)
            upstream = socket.create_connection(
                (self.config['upstream_host'], int(self.config['upstream_port'])), timeout=10)
            greeting = packet(upstream)
            if greeting[3] != 0 or greeting[4] != 10:
                raise ValueError('expected MySQL protocol 10')
            client.sendall(greeting)
            request = packet(client)
            if len(request) != 36 or request[3] != 1 or not int.from_bytes(request[4:8], 'little') & 0x800:
                raise ValueError('client must send a TLS request, never plaintext credentials')
            upstream.sendall(request)
            upstream = self.upstream_tls.wrap_socket(
                upstream, server_hostname=self.config['upstream_host'])
            actual = hashlib.sha256(upstream.getpeercert(binary_form=True)).hexdigest()
            if not hmac.compare_digest(actual, self.pin):
                raise ssl.SSLCertVerificationError('upstream leaf pin mismatch')
            # Authentication cannot reach the server until both chain and pin pass.
            client = self.local_tls.wrap_socket(client, server_side=True)
            for sock in (client, upstream):
                sock.setsockopt(socket.SOL_SOCKET, socket.SO_KEEPALIVE, 1)
                sock.setblocking(False)
            sockets = (client, upstream)
            outgoing = {sock: bytearray() for sock in sockets}
            read_needs_write, write_needs_read, ended = set(), set(), set()
            while not self.stop.is_set():
                # TLS 1.3 tickets can make the TCP socket readable without any
                # application bytes. A blocking recv here deadlocks the other
                # direction while a slower client prepares authentication.
                readable = [sock for sock in sockets if sock in write_needs_read or (
                    sock not in ended and len(outgoing[upstream if sock is client else client]) < 262144)]
                writable = [sock for sock in sockets if sock in read_needs_write or (
                    outgoing[sock] and sock not in write_needs_read)]
                pending = {sock for sock in readable if sock.pending()}
                ready_r, ready_w, _ = select.select(readable, writable, [], 0 if pending else 1)
                ready_r, ready_w = set(ready_r) | pending, set(ready_w)
                for sock in ready_w | (ready_r & write_needs_read):
                    if not outgoing[sock]:
                        continue
                    try:
                        sent = sock.send(outgoing[sock][:65536])
                        del outgoing[sock][:sent]
                        write_needs_read.discard(sock)
                    except ssl.SSLWantReadError:
                        write_needs_read.add(sock)
                    except ssl.SSLWantWriteError:
                        pass
                for source in ready_r | (ready_w & read_needs_write):
                    destination = upstream if source is client else client
                    if source in ended or len(outgoing[destination]) >= 262144:
                        continue
                    try:
                        data = source.recv(65536)
                        read_needs_write.discard(source)
                        if data:
                            outgoing[destination].extend(data)
                        else:
                            ended.add(source)
                    except ssl.SSLWantReadError:
                        pass
                    except ssl.SSLWantWriteError:
                        read_needs_write.add(source)
                if ended and not any(outgoing.values()):
                    return
        except (OSError, ValueError) as error:
            # Exception text can contain endpoints. Never log it or packet contents.
            print('connection_closed type=' + type(error).__name__, file=sys.stderr, flush=True)
        finally:
            for sock in (client, upstream):
                if sock is not None:
                    sock.close()
            self.slots.release()

    def serve(self):
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
            listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            listener.bind(('127.0.0.1', int(self.config.get('listen_port', 13306))))
            listener.listen(16)
            listener.settimeout(1)
            self.listener = listener
            print('ready loopback_only=true chain_verification=true leaf_pin=true', flush=True)
            while not self.stop.is_set():
                try:
                    client, _ = listener.accept()
                except socket.timeout:
                    continue
                if not self.slots.acquire(blocking=False):
                    client.close()
                    continue
                threading.Thread(target=self.connection, args=(client,), daemon=True).start()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('config', type=Path)
    args = parser.parse_args()
    config = json.loads(args.config.read_text())
    for key in ('upstream_ca', 'local_cert', 'local_key'):
        path = Path(config[key])
        if not path.is_absolute():
            config[key] = str(args.config.parent / path)
    relay = Relay(config)
    for sig in (signal.SIGINT, signal.SIGTERM):
        signal.signal(sig, lambda *_: relay.stop.set())
    relay.serve()


if __name__ == '__main__':
    try:
        main()
    except Exception as error:
        print('startup_failed type=' + type(error).__name__, file=sys.stderr)
        raise SystemExit(1)
