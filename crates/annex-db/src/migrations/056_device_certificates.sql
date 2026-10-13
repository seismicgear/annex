-- Public certificate bodies are immutable after verified admission. Their
-- digests are named by member-signed manifests; no private key is stored here.
ALTER TABLE member_device_keys ADD COLUMN certificate_sha256 TEXT
    CHECK(certificate_sha256 IS NULL OR (length(certificate_sha256)=64 AND certificate_sha256 NOT GLOB '*[^0-9a-f]*'));
CREATE UNIQUE INDEX member_device_certificate_digest
    ON member_device_keys(server_id,pseudonym_id,certificate_sha256)
    WHERE certificate_sha256 IS NOT NULL;

CREATE TRIGGER member_device_certificate_immutable
BEFORE UPDATE OF device_certificate,certificate_sha256 ON member_device_keys
WHEN OLD.device_certificate IS NOT NULL AND
    (NEW.device_certificate IS NOT OLD.device_certificate OR NEW.certificate_sha256 IS NOT OLD.certificate_sha256)
BEGIN
    SELECT RAISE(ABORT,'certified device cannot be replaced or downgraded');
END;

CREATE TRIGGER member_device_certificate_digest_required_insert
BEFORE INSERT ON member_device_keys
WHEN (NEW.device_certificate IS NULL) != (NEW.certificate_sha256 IS NULL)
BEGIN
    SELECT RAISE(ABORT,'certificate and digest must be stored together');
END;

CREATE TRIGGER member_device_certificate_digest_required_update
BEFORE UPDATE OF device_certificate,certificate_sha256 ON member_device_keys
WHEN (NEW.device_certificate IS NULL) != (NEW.certificate_sha256 IS NULL)
BEGIN
    SELECT RAISE(ABORT,'certificate and digest must be stored together');
END;

CREATE TRIGGER member_device_certificate_upgrade_revision
AFTER UPDATE OF device_certificate ON member_device_keys
WHEN OLD.device_certificate IS NULL AND NEW.device_certificate IS NOT NULL
BEGIN
    UPDATE channel_key_state SET revision=revision+1
    WHERE server_id=NEW.server_id AND channel_id IN (
        SELECT channel_id FROM channel_members
        WHERE server_id=NEW.server_id AND pseudonym_id=NEW.pseudonym_id
    );
END;
