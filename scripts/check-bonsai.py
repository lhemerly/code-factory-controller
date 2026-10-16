#!/usr/bin/env python3
"""Read-only, fail-closed preflight for the dedicated local Bonsai runtime."""
import argparse
import base64
import json
import os
import urllib.request
from urllib.parse import urlsplit

MODEL_ID = "bonsai-heretic-v2-27b-ptq1_0"
PROVIDER_ID = "prism-bonsai"
MODEL = f"{PROVIDER_ID}/{MODEL_ID}"


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise ValueError("redirect forbidden")


def loopback(url, path):
    parsed = urlsplit(url)
    if (parsed.scheme != "http" or parsed.hostname not in ("127.0.0.1", "::1")
            or parsed.username or parsed.password or parsed.query or parsed.fragment
            or parsed.path.rstrip("/") != path):
        raise ValueError("endpoint must be an explicit HTTP loopback URL")
    return url.rstrip("/")


def read_json(url, password=None):
    request = urllib.request.Request(url)
    if password is not None:
        token = base64.b64encode(f"opencode:{password}".encode()).decode()
        request.add_header("Authorization", f"Basic {token}")
    # Ignore proxy environment variables: neither endpoint may leave loopback.
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    with opener.open(request, timeout=10) as response:
        raw = response.read(4_194_305)
    if len(raw) > 4_194_304:
        raise ValueError("response exceeds limit")
    return json.loads(raw)


def validate_profile(config):
    if (config.get("model") != MODEL or config.get("small_model") != MODEL
            or config.get("enabled_providers") != [PROVIDER_ID]):
        raise ValueError("worker/helper must select only the approved local Bonsai model")
    for agent in config.get("agent", {}).values():
        if agent.get("model") not in (None, MODEL):
            raise ValueError("agent model override is outside the approved local identity")
    provider = config["provider"][PROVIDER_ID]
    if provider.get("npm") != "@ai-sdk/openai-compatible":
        raise ValueError("OpenAI-compatible transport required")
    url = loopback(provider["options"]["baseURL"], "/v1")
    if set(provider["models"]) != {MODEL_ID}:
        raise ValueError("only the approved Bonsai model may be configured")
    model = provider["models"][MODEL_ID]
    if model.get("id", MODEL_ID) != MODEL_ID:
        raise ValueError("API model identity differs from approved alias")
    if any(model.get("cost", {}).get(k) != 0 for k in ("input", "output", "cache_read", "cache_write")):
        raise ValueError("explicit zero API prices required")
    if model["limit"]["context"] != 8192 or model["limit"]["output"] != 1024:
        raise ValueError("profile exceeds or differs from the qualified context/output envelope")
    return url


def check(opencode_url):
    opencode_url = loopback(opencode_url, "")
    password = os.environ.get("FACTORY_OPENCODE_PASSWORD")
    health = read_json(opencode_url + "/global/health", password)
    if health.get("version") != "1.19.0" or health.get("healthy") is not True:
        raise ValueError("healthy pinned OpenCode 1.19.0 required")
    config = read_json(opencode_url + "/config", password)
    model_url = validate_profile(config)
    catalog = read_json(opencode_url + "/provider", password)
    if catalog.get("connected") != [PROVIDER_ID]:
        raise ValueError("effective connected providers must contain only prism-bonsai")
    provider = next(p for p in catalog["all"] if p["id"] == PROVIDER_ID)
    runtime_model = provider["models"][MODEL_ID]
    if runtime_model.get("status") == "deprecated":
        raise ValueError("runtime catalog marks the approved local model deprecated")
    cost = runtime_model["cost"]
    if any(cost.get(k) != 0 for k in ("input", "output")) or any(
            cost.get("cache", {}).get(k) != 0 for k in ("read", "write")):
        raise ValueError("runtime catalog must report zero API prices")
    models = read_json(model_url + "/models")
    if [m.get("id") for m in models["data"]] != [MODEL_ID]:
        raise ValueError("Prism server must expose exactly the approved Bonsai alias")
    return {"passed": True, "model": MODEL, "model_endpoint": model_url,
            "api_spend": 0, "hardware_energy_accounted": False,
            "scope": "configuration and identity only; no inference or tool qualification"}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--opencode-url", default="http://127.0.0.1:4096")
    args = parser.parse_args()
    try:
        print(json.dumps(check(args.opencode_url)))
    except (ValueError, KeyError, StopIteration, OSError) as error:
        # Endpoint errors are diagnostic; never retry another provider.
        print(json.dumps({"passed": False, "error": str(error)}))
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
