-- Add provenance tracking for federated RTX bundles (Phase 9.4).
--
-- When a bundle is relayed across federated servers, the provenance_json
-- column stores a serialized BundleProvenance with the origin_server,
-- relay_path, and bundle_id. Local bundles have NULL provenance_json.

ALTER TABLE rtx_bundles ADD COLUMN provenance_json TEXT;
