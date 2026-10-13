#!/usr/bin/env python3
"""Optional real Bark backend. Normal synthesis is strictly offline.

Install the dependencies in a dedicated Python environment, run --prepare once,
then configure an executable launcher for this script as ANNEX_BARK_BINARY_PATH.
Stdout contains only native 24 kHz mono signed 16-bit little-endian PCM.
"""
import argparse
import contextlib
import hashlib
import json
import os
from pathlib import Path
import re
import sys
import time

DEFAULT_MODEL = "suno/bark-small"
DEFAULT_REVISION = "1dbd7a128513b8ae4a4e2130fed57b7ac9da5bcd"


def arguments():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--text")
    parser.add_argument("--output_raw", action="store_true")
    parser.add_argument("--prepare", action="store_true", help="Explicitly download the pinned model; never synthesizes")
    parser.add_argument("--check", action="store_true", help="Check pinned local model and preset files without synthesis or downloads")
    parser.add_argument("--model", default=os.environ.get("ANNEX_BARK_MODEL", DEFAULT_MODEL))
    parser.add_argument("--revision", default=os.environ.get("ANNEX_BARK_REVISION", DEFAULT_REVISION))
    parser.add_argument("--speaker", type=int, default=None, help="Optional English v2 speaker preset, 0 through 9")
    parser.add_argument("--threads", type=int, default=int(os.environ.get("ANNEX_BARK_THREADS", "4")))
    args = parser.parse_args()
    if not re.fullmatch(r"[0-9a-f]{40}", args.revision):
        parser.error("revision must be an immutable 40-character model commit")
    if args.model != DEFAULT_MODEL and args.revision == DEFAULT_REVISION:
        parser.error("a different model requires its own explicit immutable revision")
    if args.speaker is not None and not 0 <= args.speaker <= 9:
        parser.error("speaker must be between 0 and 9")
    if not 1 <= args.threads <= 32:
        parser.error("threads must be between 1 and 32")
    if args.prepare and args.check:
        parser.error("preparation and checking are separate operations")
    if args.prepare or args.check:
        if args.text is not None or args.output_raw:
            parser.error("preparation and synthesis are separate operations")
    elif not args.output_raw or not args.text or not args.text.strip() or len(args.text.encode("utf-8")) > 65536:
        parser.error("synthesis requires --output_raw and 1 to 65536 bytes of nonblank --text")
    return args


def check_cached_model(args):
    """Validate the exact offline selection without importing/loading Torch."""
    from huggingface_hub import hf_hub_download, try_to_load_from_cache

    def existing(name):
        if not isinstance(name, str) or name.startswith(("/", "\\")) or ".." in name.replace("\\", "/").split("/"):
            raise ValueError("Invalid pinned model file path")
        path = hf_hub_download(args.model, name, revision=args.revision, local_files_only=True)
        if not Path(path).is_file() or Path(path).stat().st_size == 0:
            raise ValueError("A required pinned model file is unavailable")
        return Path(path)

    config = json.loads(existing("config.json").read_text(encoding="utf-8"))
    generation = json.loads(existing("generation_config.json").read_text(encoding="utf-8"))
    if config.get("model_type") != "bark" or generation.get("sample_rate") != 24000:
        raise ValueError("The configured snapshot is not a supported 24000 Hz Bark model")
    existing("tokenizer_config.json")
    existing("tokenizer.json")
    weight_files = []
    for name in ("model.safetensors", "pytorch_model.bin"):
        cached = try_to_load_from_cache(args.model, name, revision=args.revision)
        if isinstance(cached, str) and Path(cached).is_file():
            existing(name)
            weight_files.append(name)
            break
    if not weight_files:
        for name in ("model.safetensors.index.json", "pytorch_model.bin.index.json"):
            cached = try_to_load_from_cache(args.model, name, revision=args.revision)
            if isinstance(cached, str) and Path(cached).is_file():
                index = json.loads(existing(name).read_text(encoding="utf-8"))
                weight_files = sorted(set(index.get("weight_map", {}).values()))
                if not weight_files:
                    raise ValueError("Pinned Bark model has an empty weight index")
                for filename in weight_files:
                    existing(filename)
                break
    if not weight_files:
        raise ValueError("Pinned Bark weights are not installed; run explicit preparation first")
    if args.speaker is not None:
        metadata = json.loads(existing("speaker_embeddings_path.json").read_text(encoding="utf-8"))
        preset = metadata.get(f"v2/en_speaker_{args.speaker}")
        if not isinstance(preset, dict):
            raise ValueError("The selected pinned model has no such speaker preset")
        for field in ("semantic_prompt", "coarse_prompt", "fine_prompt"):
            existing(preset.get(field))
    return {"model": args.model, "revision": args.revision, "speaker": args.speaker,
            "sample_rate": 24000, "operation": "check", "weights_present": True}


def voice_preset(processor, args, np):
    if args.speaker is None:
        return None
    from huggingface_hub import hf_hub_download
    preset = f"v2/en_speaker_{args.speaker}"
    entries = processor.speaker_embeddings.get(preset)
    if not entries:
        raise ValueError("The selected model does not contain this voice preset")
    result = {}
    for field in ("semantic_prompt", "coarse_prompt", "fine_prompt"):
        filename = entries[field]
        # Paths come from pinned model metadata, but still cannot leave its repo.
        if not isinstance(filename, str) or filename.startswith(("/", "\\")) or ".." in filename.replace("\\", "/").split("/"):
            raise ValueError("Invalid voice preset metadata path")
        path = hf_hub_download(args.model, filename, revision=args.revision, local_files_only=not args.prepare)
        result[field] = np.load(path, allow_pickle=False)
    return result


def main():
    args = arguments()
    if not args.prepare:
        # Includes any nested processor/preset lookup; a voice request must never
        # download or execute a newer model or require an external service.
        os.environ["HF_HUB_OFFLINE"] = "1"
    if args.check:
        print(json.dumps(check_cached_model(args)))
        return
    os.environ["TOKENIZERS_PARALLELISM"] = "false"
    started = time.monotonic()
    with contextlib.redirect_stdout(sys.stderr):
        import numpy as np
        import torch
        from transformers import AutoProcessor, BarkModel
        torch.set_num_threads(args.threads)
        processor = AutoProcessor.from_pretrained(args.model, revision=args.revision,
            trust_remote_code=False, local_files_only=not args.prepare)
        preset = voice_preset(processor, args, np)
        model = BarkModel.from_pretrained(args.model, revision=args.revision,
            trust_remote_code=False, local_files_only=not args.prepare)
        model.eval()
        rate = int(model.generation_config.sample_rate)
        if rate != 24000:
            raise ValueError("Bark backend requires native 24000 Hz output")
        receipt = {"model": args.model, "revision": args.revision, "sample_rate": rate,
                   "speaker": args.speaker, "device": str(model.device), "operation": "prepare" if args.prepare else "synthesize"}
        if args.prepare:
            print(json.dumps(receipt), file=sys.stderr)
            return
        tokens = processor.tokenizer(args.text, add_special_tokens=False)["input_ids"]
        if len(tokens) > 256:
            raise ValueError("Bark accepts at most 256 text tokens per intent; split this message into shorter speech intents")
        inputs = processor(args.text, voice_preset=preset, return_tensors="pt", truncation=False)
        with torch.inference_mode():
            samples = model.generate(**inputs).cpu().numpy().squeeze()
        if samples.ndim != 1 or not 0 < len(samples) <= rate * 60 or not np.isfinite(samples).all():
            raise ValueError("Bark returned invalid or excessive audio")
        pcm = (np.clip(samples, -1, 1) * 32767).astype("<i2").tobytes()
        receipt.update(text_sha256=hashlib.sha256(args.text.encode("utf-8")).hexdigest(),
                       pcm_sha256=hashlib.sha256(pcm).hexdigest(), pcm_bytes=len(pcm),
                       elapsed_seconds=round(time.monotonic() - started, 3))
        print(json.dumps(receipt), file=sys.stderr)
        audit_path = os.environ.get("ANNEX_BARK_AUDIT_LOG")
        if audit_path:
            with Path(audit_path).open("a", encoding="utf-8") as audit:
                audit.write(json.dumps(receipt) + "\n")
    sys.stdout.buffer.write(pcm)


if __name__ == "__main__":
    main()
