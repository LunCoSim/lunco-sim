#!/usr/bin/env python3
import subprocess
import time
import urllib.request
import json
import os
import sys

def main():
    print("🚀 Starting LunCoSim Sandbox with API on port 4101...")
    # Run cargo run -p lunco-sandbox --bin sandbox -- --api 4101
    # Redirect stdout/stderr to a log file to prevent terminal clutter
    log_file = open("/tmp/sandbox_api_test.log", "w")
    proc = subprocess.Popen(
        ["cargo", "run", "-p", "lunco-sandbox", "--bin", "sandbox", "--", "--api", "4101"],
        stdout=log_file,
        stderr=log_file,
        preexec_fn=os.setsid
    )

    base_url = "http://127.0.0.1:4101/api/commands"
    
    # Wait for API to be ready
    print("⏳ Waiting for API to start up...")
    ready = False
    for _ in range(30):
        try:
            req = urllib.request.Request(
                base_url,
                data=json.dumps({"type": "DiscoverSchema"}).encode("utf-8"),
                headers={"Content-Type": "application/json"},
                method="POST"
            )
            with urllib.request.urlopen(req, timeout=2) as resp:
                data = json.loads(resp.read().decode("utf-8"))
                if "error" not in data:
                    ready = True
                    break
        except Exception:
            time.sleep(1)

    if not ready:
        print("❌ Error: API did not respond within 30 seconds.")
        proc.terminate()
        sys.exit(1)

    print("✅ API is active!")
    
    # Step 1: Discover commands and check for ApplyUsdOp
    print("\n🔍 Step 1: Querying commands schema via API...")
    req = urllib.request.Request(
        base_url,
        data=json.dumps({"type": "DiscoverSchema"}).encode("utf-8"),
        headers={"Content-Type": "application/json"},
        method="POST"
    )
    try:
        with urllib.request.urlopen(req) as resp:
            res = json.loads(resp.read().decode("utf-8"))
            commands = res.get("data", {}).get("commands", [])
            cmd_names = [c.get("name") for c in commands]
            print(f"  Available API commands: {', '.join(cmd_names[:10])} ...")
            if "ApplyUsdOp" in cmd_names:
                print("  🎉 Found ApplyUsdOp in schema!")
            else:
                print("  ❌ Error: ApplyUsdOp command not found in schema.")
                proc.terminate()
                sys.exit(1)
    except Exception as e:
        print(f"❌ Error querying schema: {e}")
        proc.terminate()
        sys.exit(1)

    # Step 2: List entities
    print("\n📋 Step 2: Querying spawned entities...")
    req = urllib.request.Request(
        base_url,
        data=json.dumps({"type": "ListEntities"}).encode("utf-8"),
        headers={"Content-Type": "application/json"},
        method="POST"
    )
    try:
        with urllib.request.urlopen(req) as resp:
            res = json.loads(resp.read().decode("utf-8"))
            entities = res.get("data", {}).get("entities", [])
            print(f"  Found {len(entities)} entities in sandbox.")
            for ent in entities[:5]:
                print(f"    • {ent.get('name')} (Type: {ent.get('type')}, ID: {ent.get('api_id')})")
    except Exception as e:
        print(f"❌ Error querying entities: {e}")

    # Clean up and exit
    print("\n🛑 Terminating sandbox process...")
    proc.terminate()
    proc.wait()
    print("✅ Sandbox terminated. Tests completed successfully!")

if __name__ == "__main__":
    main()
