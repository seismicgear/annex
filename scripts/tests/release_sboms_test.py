import importlib.util
import json
import pathlib
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("combine_sboms", pathlib.Path(__file__).parents[1] / "combine-rust-sboms.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class ReleaseInventoryTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="annex-sbom-test-")
        self.addCleanup(self.temporary.cleanup)
        self.directory = pathlib.Path(self.temporary.name)
        self.inputs = self.directory / "inputs"
        self.inputs.mkdir()
        self.output = self.directory / "union.json"
        self.metadata = {"packages": [], "workspace_members": []}
        for i in range(12):
            name, identity = f"annex-member-{i:02}", f"path:annex-member-{i:02}"
            self.metadata["packages"].append({"name": name, "version": "0.1.0", "id": identity, "source": None})
            self.metadata["workspace_members"].append(identity)
            doc = {
                "bomFormat": "CycloneDX", "specVersion": "1.4", "version": 1,
                "metadata": {"component": {"name": name, "version": "0.1.0", "bom-ref": identity, "type": "application"}},
                "components": [{"name": "glib", "version": "0.18.5", "bom-ref": "path:glib", "type": "library"}],
                "dependencies": [{"ref": identity, "dependsOn": ["path:glib"]}, {"ref": "path:glib", "dependsOn": []}],
            }
            (self.inputs / f"{name}.json").write_text(json.dumps(doc), encoding="utf-8")
        self.metadata["packages"].append({"name": "glib", "version": "0.18.5", "id": "path:glib", "source": None})
        self.backports = {"crates": [{"name": "glib", "version": "0.18.5", "path": "vendor/glib", "archive": {"sha256": "upstream-digest"}, "patches": [{"path": "src/variant_iter.rs", "patched_sha256": "patched-digest"}]}]}

    def run_combine(self):
        module.combine(self.inputs, self.output, self.metadata, self.backports, "a" * 40, "manifest-digest")
        return json.loads(self.output.read_text(encoding="utf-8"))

    def change_first(self, mutate):
        filename = next(self.inputs.glob("*.json"))
        doc = json.loads(filename.read_text(encoding="utf-8"))
        mutate(doc)
        filename.write_text(json.dumps(doc), encoding="utf-8")

    def test_all_twelve_roots_graph_and_exact_backport_provenance_survive(self):
        result = self.run_combine()
        self.assertEqual(len(result["components"]), 13)
        self.assertEqual(len(result["dependencies"]), 13)
        glib = next(component for component in result["components"] if component["name"] == "glib")
        properties = {item["name"]: item["value"] for item in glib["properties"]}
        self.assertEqual(properties["annex:source-kind"], "reviewed-upstream-backport")
        self.assertEqual(properties["annex:source-revision"], "a" * 40)
        self.assertEqual(properties["annex:upstream-archive-sha256"], "upstream-digest")
        self.assertEqual(properties["annex:backport-manifest-sha256"], "manifest-digest")
        self.assertEqual(json.loads(properties["annex:patches"]), self.backports["crates"][0]["patches"])

    def test_missing_one_of_twelve_crates_is_refused(self):
        next(self.inputs.glob("*.json")).unlink()
        with self.assertRaisesRegex(ValueError, "inventory mismatch"):
            self.run_combine()

    def test_wrong_crate_root_is_refused(self):
        self.change_first(lambda doc: doc["metadata"]["component"].update(name="other"))
        with self.assertRaisesRegex(ValueError, "inventory root"):
            self.run_combine()

    def test_registry_component_cannot_masquerade_as_backport(self):
        self.change_first(lambda doc: doc["components"][0].update({"bom-ref": "registry:glib"}))
        with self.assertRaisesRegex(ValueError, "unreviewed source"):
            self.run_combine()

    def test_unresolved_dependency_reference_is_refused(self):
        self.change_first(lambda doc: doc["dependencies"][0]["dependsOn"].append("missing-package"))
        with self.assertRaisesRegex(ValueError, "unresolved references"):
            self.run_combine()

    def test_missing_backport_is_refused(self):
        self.metadata["packages"].append({"name": "ark-relations", "version": "0.5.1", "id": "path:ark", "source": None})
        self.backports["crates"].append({"name": "ark-relations", "version": "0.5.1"})
        with self.assertRaisesRegex(ValueError, "omits a reviewed backport"):
            self.run_combine()

    def test_distinct_sources_of_same_name_version_remain_distinct(self):
        self.change_first(lambda doc: doc["components"].extend([
            {"name": "same", "version": "1", "bom-ref": "registry:same", "type": "library"},
            {"name": "same", "version": "1", "bom-ref": "path:same", "type": "library"},
        ]))
        result = self.run_combine()
        self.assertEqual(len([c for c in result["components"] if c["name"] == "same"]), 2)


if __name__ == "__main__":
    unittest.main()
