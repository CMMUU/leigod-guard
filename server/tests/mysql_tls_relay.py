"""Prove the TLS relay never forwards credentials after a chain/pin failure."""
import hashlib
import importlib.util
import socket
import ssl
import subprocess
import tempfile
import threading
import time
from pathlib import Path

root = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('relay', root/'deploy/server/mysql-tls-relay.py')
module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module)


def port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


with tempfile.TemporaryDirectory() as folder:
    folder = Path(folder)
    for name in ('upstream', 'other', 'local'):
        subprocess.run(['openssl','req','-x509','-newkey','rsa:2048','-nodes','-days','1',
                        '-subj','/CN=localhost','-addext','subjectAltName=DNS:localhost,IP:127.0.0.1',
                        '-keyout',str(folder/(name+'.key')),'-out',str(folder/(name+'.pem'))],
                       stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,check=True)
    der = ssl.PEM_cert_to_DER_cert((folder/'upstream.pem').read_text())
    pin = hashlib.sha256(der).hexdigest()
    for case in ('success', 'wrong_pin', 'wrong_ca', 'plaintext'):
        upstream_port, local_port = port(), port()
        config = dict(upstream_host='127.0.0.1', upstream_port=upstream_port,
                      upstream_ca=str(folder/('other.pem' if case == 'wrong_ca' else 'upstream.pem')),
                      upstream_leaf_sha256='0'*64 if case == 'wrong_pin' else pin,
                      local_cert=str(folder/'local.pem'),local_key=str(folder/'local.key'),
                      listen_port=local_port)
        relay = module.Relay(config)
        observed = []
        ready = threading.Event()
        def upstream():
            ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
            ctx.load_cert_chain(folder/'upstream.pem',folder/'upstream.key')
            with socket.socket() as listener:
                listener.bind(('127.0.0.1',upstream_port));listener.listen(1);listener.settimeout(5);ready.set()
                conn, _ = listener.accept()
                try:
                    conn.settimeout(5)
                    # The relay only transports this greeting; a real driver test follows at deployment.
                    greeting = b'\x0a8.4.0\x00' + b'fixture'
                    conn.sendall(len(greeting).to_bytes(3,'little')+b'\x00'+greeting)
                    module.packet(conn)
                    conn = ctx.wrap_socket(conn,server_side=True)
                    data = conn.recv(1024)
                    if data:
                        observed.append(data);conn.sendall(data)
                except (OSError,ValueError):
                    pass
                finally:
                    conn.close()
        server = threading.Thread(target=upstream);server.start();assert ready.wait(3)
        worker = threading.Thread(target=relay.serve);worker.start()
        for _ in range(50):
            if relay.listener is not None:break
            time.sleep(.02)
        client = socket.create_connection(('127.0.0.1',local_port),timeout=5)
        try:
            module.packet(client)
            request=(0 if case == 'plaintext' else 0x800).to_bytes(4,'little')+bytes(28)
            client.sendall(b'\x20\x00\x00\x01'+request)
            if case == 'plaintext':
                assert client.recv(1)==b''
            else:
                ctx=ssl.create_default_context(cafile=str(folder/'local.pem'))
                client=ctx.wrap_socket(client,server_hostname='localhost')
                client.sendall(b'fixture-auth-secret')
                assert client.recv(1024)==b'fixture-auth-secret'
                assert case == 'success'
        except OSError:
            assert case != 'success'
        finally:
            client.close();server.join(7);relay.stop.set();worker.join(3)
        assert observed == ([b'fixture-auth-secret'] if case == 'success' else []), case
        print('PASS TLS relay '+case+' authentication forwarding verified')
