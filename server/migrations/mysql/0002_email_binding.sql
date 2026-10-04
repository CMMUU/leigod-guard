-- Binding proofs are separate from login proofs: neither endpoint can redeem the other.
CREATE TABLE email_binding_challenges (
 user_id BINARY(16) PRIMARY KEY,
 email VARCHAR(254) COLLATE utf8mb4_0900_as_cs NOT NULL,
 request_id BINARY(16) NOT NULL UNIQUE, code_hash VARBINARY(32) NOT NULL,
 requested_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
 expires_at TIMESTAMP(6) NOT NULL, attempts INT NOT NULL DEFAULT 0,
 ready BOOLEAN NOT NULL DEFAULT FALSE,
 FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE,
 INDEX email_binding_address_idx(email,requested_at),
 INDEX email_binding_expiry_idx(expires_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
