#!/usr/bin/env python3
import subprocess
import time
import urllib.request
import urllib.error
import json
import os
import sys

def main():
    temp_usda_path = "/tmp/test_http_usd.usda"
    
    # 1. Create a temporary USDA file with initial empty scaffold
    scaffold = """#usda 1.0
(
    defaultPrim = "World"
)

def Xform "World"
{
}
"""
    with open(temp_usda_path, "w") as f:
        f.write(scaffold)
        
    print(f"📝 Created initial USDA scaffold at {temp_usda_path}")

    # 2. Start the LunCoSim Sandbox
    print("🚀 Starting LunCoSim Sandbox with API on port 4101...")
    log_file = open("/tmp/sandbox_http_apply_test.log", "w")
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

    # 3. Open the temporary USD document via API
    print(f"\n📂 Step 1: Requesting to open {temp_usda_path} via API...")
    open_req = urllib.request.Request(
        base_url,
        data=json.dumps({
            "type": "OpenFile",
            "path": temp_usda_path
        }).encode("utf-8"),
        headers={"Content-Type": "application/json"},
        method="POST"
    )
    try:
        with urllib.request.urlopen(open_req) as resp:
            res = json.loads(resp.read().decode("utf-8"))
            print(f"  OpenFile response: {res}")
    except Exception as e:
        print(f"❌ Error opening file: {e}")
        proc.terminate()
        sys.exit(1)

    # Wait for the async load pipeline to complete in the sandbox
    time.sleep(3)

    # 4. List open documents to retrieve the doc_id
    print("\n📋 Step 2: Querying open documents list...")
    list_req = urllib.request.Request(
        base_url,
        data=json.dumps({
            "type": "ListOpenDocuments"
        }).encode("utf-8"),
        headers={"Content-Type": "application/json"},
        method="POST"
    )
    doc_id = None
    try:
        with urllib.request.urlopen(list_req) as resp:
            res = json.loads(resp.read().decode("utf-8"))
            open_docs = res.get("data", {}).get("open_documents", [])
            print(f"  Open documents: {open_docs}")
            for doc in open_docs:
                if doc.get("title") == "test_http_usd.usda":
                    doc_id = doc.get("doc_id")
                    break
    except urllib.error.HTTPError as e:
        print(f"❌ HTTP Error listing documents: {e.code} {e.reason}")
        try:
            err_body = e.read().decode("utf-8")
            print(f"  Response body: {err_body}")
        except Exception:
            pass
        proc.terminate()
        sys.exit(1)
    except Exception as e:
        print(f"❌ Error listing documents: {e}")
        proc.terminate()
        sys.exit(1)

    if doc_id is None:
        print("❌ Error: test_http_usd.usda was not found in open documents registry.")
        proc.terminate()
        sys.exit(1)

    print(f"  🎉 Found test document with ID: {doc_id}")

    # 5. Apply UsdOp: AddPrim
    print(f"\n➕ Step 3: Triggering ApplyUsdOp (AddPrim: Cube) on doc {doc_id}...")
    add_prim_req = urllib.request.Request(
        base_url,
        data=json.dumps({
            "type": "ApplyUsdOp",
            "doc": doc_id,
            "op": {
                "AddPrim": {
                    "edit_target": "@root@",
                    "parent_path": "/World",
                    "name": "TestCube",
                    "type_name": "Cube"
                }
            }
        }).encode("utf-8"),
        headers={"Content-Type": "application/json"},
        method="POST"
    )
    try:
        with urllib.request.urlopen(add_prim_req) as resp:
            res = json.loads(resp.read().decode("utf-8"))
            print(f"  ApplyUsdOp (AddPrim) response: {res}")
    except Exception as e:
        print(f"❌ Error applying AddPrim: {e}")
        proc.terminate()
        sys.exit(1)

    time.sleep(1)

    # 6. Apply UsdOp: SetAttribute
    print(f"\n🔧 Step 4: Triggering ApplyUsdOp (SetAttribute: size = 7.5) on doc {doc_id}...")
    set_attr_req = urllib.request.Request(
        base_url,
        data=json.dumps({
            "type": "ApplyUsdOp",
            "doc": doc_id,
            "op": {
                "SetAttribute": {
                    "edit_target": "@root@",
                    "path": "/World/TestCube",
                    "name": "size",
                    "type_name": "double",
                    "value": "7.5"
                }
            }
        }).encode("utf-8"),
        headers={"Content-Type": "application/json"},
        method="POST"
    )
    try:
        with urllib.request.urlopen(set_attr_req) as resp:
            res = json.loads(resp.read().decode("utf-8"))
            print(f"  ApplyUsdOp (SetAttribute) response: {res}")
    except Exception as e:
        print(f"❌ Error applying SetAttribute: {e}")
        proc.terminate()
        sys.exit(1)

    time.sleep(1)

    # 7. Save Document
    print(f"\n💾 Step 5: Triggering SaveDocument for doc {doc_id}...")
    save_req = urllib.request.Request(
        base_url,
        data=json.dumps({
            "type": "SaveDocument",
            "doc": doc_id
        }).encode("utf-8"),
        headers={"Content-Type": "application/json"},
        method="POST"
    )
    try:
        with urllib.request.urlopen(save_req) as resp:
            res = json.loads(resp.read().decode("utf-8"))
            print(f"  SaveDocument response: {res}")
    except Exception as e:
        print(f"❌ Error saving document: {e}")
        proc.terminate()
        sys.exit(1)

    time.sleep(2)

    # Clean up and exit
    print("\n🛑 Terminating sandbox process...")
    proc.terminate()
    proc.wait()
    print("✅ Sandbox terminated.")

    # 8. Verify target file contents
    print("\n🔍 Step 6: Verifying final file contents on disk...")
    with open(temp_usda_path, "r") as f:
        content = f.read()
        print("--- File Contents Start ---")
        print(content)
        print("--- File Contents End ---")

        if 'def Cube "TestCube"' in content and 'double size = 7.5' in content:
            print("\n🎉 SUCCESS: The ApplyUsdOp command successfully mutated the stage and saved modifications back to disk!")
            sys.exit(0)
        else:
            print("\n❌ FAILURE: The file did not contain the expected prim definitions or attribute values.")
            sys.exit(1)

if __name__ == "__main__":
    main()
