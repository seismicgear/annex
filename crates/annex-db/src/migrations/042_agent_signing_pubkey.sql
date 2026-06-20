-- Per-agent Ed25519 signing public key, captured at VRP handshake.
--
-- Closes the remaining half of AUDIT P4-FED-1: the RTX bundle `signature`
-- field was only length-checked, never cryptographically verified, because no
-- agent signing key was on file. Agents now advertise an Ed25519 public key
-- (64-char hex) during the VRP agent handshake; the RTX publish path verifies
-- the bundle's author signature against it. NULL = legacy agent that has not
-- yet advertised a key (author signature is not enforced for it).
ALTER TABLE agent_registrations ADD COLUMN signing_pubkey TEXT;
