import sys
import json
import urllib.request
import urllib.error
import shlex

API_URL = "http://localhost:8082/api/command"

def send_command(target, command, args):
    payload = {
        "name": command,
        "target_path": target,
        "arguments": args
    }
    
    try:
        data = json.dumps(payload).encode('utf-8')
        req = urllib.request.Request(
            API_URL, 
            data=data, 
            headers={'Content-Type': 'application/json'}
        )
        
        with urllib.request.urlopen(req) as response:
            result = response.read().decode('utf-8')
            return f"Success: {result}"
            
    except urllib.error.URLError as e:
        return f"Error connecting to simulation: {e}"
    except Exception as e:
        return f"Error: {e}"

def get_command_list():
    try:
        req = urllib.request.Request(API_URL)
        with urllib.request.urlopen(req) as response:
            data = json.loads(response.read().decode('utf-8'))
            if "targets" in data:
                return data["targets"]
            return {}
    except Exception as e:
        print(f"Error fetching command list: {e}")
        return {}

def parse_args(arg_list):
    args = {}
    for item in arg_list:
        if '=' in item:
            key, val = item.split('=', 1)
            # Try to convert to number or bool
            if val.lower() == 'true': val = True
            elif val.lower() == 'false': val = False
            else:
                try:
                    if '.' in val: val = float(val)
                    else: val = int(val)
                except ValueError:
                    pass # Keep as string
            args[key] = val
        else:
            # Handle positional args or flags if needed, for now treat as value=True?
            # Or just ignore/error. Sticking to key=value for now as per console syntax
            pass 
    return args

def main():
    print(f"LunCo Remote Console connecting to {API_URL}")
    print("Syntax: Target COMMAND arg1=val1 arg2=val2")
    print("Example: Rover1 SET_MOTOR value=0.5")
    print("Type 'exit' or 'quit' to close.")
    print("-" * 40)

    while True:
        try:
            line = input("> ").strip()
            if not line:
                continue
                
            if line.lower() in ['exit', 'quit']:
                break
                
            # Parse input
            try:
                parts = shlex.split(line)
            except ValueError as e:
                print(f"Parse error: {e}")
                continue

            if len(parts) < 2:
                if len(parts) == 1 and (parts[0].lower() == "list" or parts[0].lower() == "/list"):
                    # Handle bare 'list' command
                    parts.append("LIST") # Dummy command to pass check
                else:
                    print("Invalid syntax. Need at least Target and Command.")
                    continue
                
            target = parts[0]
            command = parts[1]
            raw_args = parts[2:]
            
            args = parse_args(raw_args)
            
            # Special aliases for ease of use
            if command.upper() == "LIST" or target.upper() == "LIST" or target.upper() == "/LIST":
                 targets = get_command_list()
                 if not targets:
                     print("No commandable targets found.")
                 else:
                     print("\nAvailable Targets:")
                     for t in targets:
                         print(f"  {t}")
                         # Optional: Print commands for each target?
                         # for cmd in targets[t]:
                         #    print(f"    - {cmd['name']}")
                 continue

            result = send_command(target, command, args)
            print(result)

        except KeyboardInterrupt:
            print("\nExiting...")
            break
        except Exception as e:
            print(f"Unexpected error: {e}")

if __name__ == "__main__":
    main()
