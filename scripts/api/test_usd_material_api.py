#!/usr/bin/env python3
import os

from runtime import ProductionSession

def main():
    port = int(os.environ.get("LUNCOSIM_API_PORT", "4101"))
    print(f"🚀 Starting production luncosim with API on port {port}...")
    with ProductionSession(port) as session:
        print("✅ Runtime is ready.")
        print("\n🔍 Querying commands schema via API...")
        response = session.post({"type": "DiscoverSchema"})
        if response.get("error"):
            raise RuntimeError(f"DiscoverSchema failed: {response}")
        commands = response.get("data", {}).get("commands", [])
        command_names = {command.get("name") for command in commands}
        if "ApplyUsdOp" not in command_names:
            raise RuntimeError("ApplyUsdOp is absent from the live command schema")
        print(f"  Found {len(command_names)} commands; ApplyUsdOp is registered.")

        print("\n📋 Querying entities through the command funnel...")
        response = session.post({"type": "ListEntities"})
        if response.get("error"):
            raise RuntimeError(f"ListEntities failed: {response}")
        entities = response.get("data", {}).get("entities", [])
        print(f"  Found {len(entities)} entities.")
        for entity in entities[:5]:
            print(f"    • {entity.get('name')} (type: {entity.get('type')}, id: {entity.get('api_id')})")

    print("✅ Production runtime shut down through ExecuteCommand.Exit and released its API port.")

if __name__ == "__main__":
    main()
