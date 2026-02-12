import sys
import json
import urllib.request
import urllib.error
import shlex
import os
import atexit

try:
    import readline
except ImportError:
    # Readline is not available on some platforms (like Windows without pyreadline)
    readline = None

API_URL = "http://localhost:8082/api/command"
HISTORY_FILE = "remote_console_commands_history.txt"

def save_history():
    if readline:
        try:
            readline.write_history_file(HISTORY_FILE)
        except Exception as e:
            print(f"Error saving history: {e}")

if readline:
    if os.path.exists(HISTORY_FILE):
        try:
            readline.read_history_file(HISTORY_FILE)
        except Exception as e:
            print(f"Error loading history: {e}")
    atexit.register(save_history)

class RemoteCompleter:
    def __init__(self):
        self.targets_cache = {}
        self.options = []

    def refresh(self):
        self.targets_cache = get_command_list()

    def complete(self, text, state):
        if state == 0:
            buffer = readline.get_line_buffer()
            parts = shlex.split(buffer) if buffer else []
            
            # If we are completing target name
            if not buffer or (len(parts) == 1 and not buffer.endswith(" ")):
                if not self.targets_cache:
                    self.refresh()
                self.options = [t for t in self.targets_cache.keys() if t.startswith(text)]
                self.options.extend([s for s in ["list", "entities", "exit", "quit"] if s.startswith(text)])
            
            # If we are completing command for a target
            elif len(parts) >= 1:
                target = parts[0]
                if target in self.targets_cache:
                    cmd_to_complete = parts[1] if len(parts) > 1 else ""
                    self.options = [c["name"] for c in self.targets_cache[target] if c["name"].startswith(text.upper())]
                else:
                    self.options = []
            else:
                self.options = []

        if state < len(self.options):
            return self.options[state]
        else:
            return None

completer = RemoteCompleter()
if readline:
    readline.set_completer(completer.complete)
    readline.parse_and_bind("tab: complete")
    # Make delimiters include space but NOT slash or path characters
    readline.set_completer_delims(' \t\n')

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

def _convert_val(val):
    if val.lower() == 'true': return True
    elif val.lower() == 'false': return False
    try:
        if '.' in val: return float(val)
        else: return int(val)
    except ValueError:
        return val # Keep as string

def parse_args(arg_list):
    args = {}
    for item in arg_list:
        if '=' in item:
            key, val = item.split('=', 1)
            args[key] = _convert_val(val)
        else:
            # Handle as a positional value stored in 'value'
            args["value"] = _convert_val(item)
    return args

def main():
    print(f"LunCo Remote Console connecting to {API_URL}")
    print("Syntax: Target COMMAND(args) or Target COMMAND arg1=val1")
    print("Example: Simulation SPAWN(Spacecraft)")
    print("         Avatar KEY_DOWN(w)")
    print("         Rover1 SET_MOTOR(0.5)")
    print("Type 'list' to see targets, 'Target list' for commands.")
    if readline:
        print("Use Tab for autocompletion and Up/Down arrows for history.")
        print(f"Command history is saved to {HISTORY_FILE}")
    print("Type 'exit' or 'quit' to close.")
    print("-" * 40)

    while True:
        try:
            line = input("> ").strip()
            if not line:
                continue
                
            if line.lower() in ['exit', 'quit']:
                break
                
            # Improved Parsing to support:
            # 1. Target COMMAND arg1=val1
            # 2. Target COMMAND(arg1=val1, arg2=val2)
            # 3. Target COMMAND(value)
            # 4. list or list() (shortcut)
            
            import re
            
            # Handle shorthand commands first
            line_clean = line.strip()
            line_no_space = line_clean.replace(" ", "").lower()
            
            if line_clean.lower() in ["list", "/list"] or line_no_space in ["list()", "/list()"]:
                target, command, paren_args, trailing_args = ("LIST", "LIST", "", "")
            elif line_clean.lower() == "entities" or line_no_space in ["entities()", "entities"]:
                target, command, paren_args, trailing_args = ("ENTITIES", "ENTITIES", "", "")
            else:
                # Regex to match target, command, and optional parentheses content or trailing args
                # Group 1: Target, Group 2: Command, Group 3: Parentheses content, Group 4: Trailing args
                match = re.match(r'^([^\s(]+)\s+([^\s(]+)(?:\((.*)\))?\s*(.*)$', line_clean)
                
                if not match:
                    print("Invalid syntax. Use: Target COMMAND(args) or Target COMMAND args")
                    continue
                
                target, command, paren_args, trailing_args = match.groups()
            
            # Combine args from parentheses and trailing
            all_raw_args = []
            if paren_args:
                # Basic split by comma, ignoring commas inside quotes would be better but let's keep it simple for now
                all_raw_args.extend([a.strip() for a in paren_args.split(',') if a.strip()])
            if trailing_args:
                try:
                    all_raw_args.extend(shlex.split(trailing_args))
                except ValueError:
                    all_raw_args.extend(trailing_args.split())

            raw_args = all_raw_args
            args = parse_args(raw_args)
            
            # Smart mapping: if there's only one argument and it's not key=val, map it to a default key
            if len(raw_args) == 1 and '=' not in raw_args[0]:
                val = args.get('value', raw_args[0])
                # Default keys for common commands
                defaults = {
                    "SPAWN": "type",
                    "TAKE_CONTROL": "target",
                    "KEY_DOWN": "key",
                    "KEY_UP": "key",
                    "KEY_PRESS": "key"
                }
                default_key = defaults.get(command.upper(), "value")
                args = {default_key: _convert_val(str(val))}
            
            # Remove 'value' if it was added by parse_args but we didn't use it
            if "value" in args and len(args) > 1:
                args.pop("value")
            
            # Special aliases for ease of use
            if command.upper() == "LIST" or target.upper() == "LIST" or target.upper() == "/LIST":
                 targets = get_command_list()
                 if not targets:
                     print("No commandable targets found.")
                 else:
                     is_global = target.upper() in ["LIST", "/LIST"]
                     
                     if is_global:
                         print("\nAvailable Targets:")
                         for t in targets:
                             cmds = ", ".join([c['name'] for c in targets[t]])
                             print(f"  {t} -> [{cmds}]")
                     else:
                         # Specific target list
                         if target in targets:
                             print(f"\nCommands for {target}:")
                             for cmd in targets[target]:
                                 args_str = ", ".join([f"{a['name']}:{a['type']}" for a in cmd['arguments']])
                                 print(f"  {cmd['name']}({args_str})")
                         else:
                             # Try fuzzy match
                             found = False
                             for t in targets:
                                 if t.lower() == target.lower():
                                     print(f"\nCommands for {t}:")
                                     for cmd in targets[t]:
                                         args_str = ", ".join([f"{a['name']}:{a['type']}" for a in cmd['arguments']])
                                         print(f"  {cmd['name']}({args_str})")
                                     found = True
                                     break
                             if not found:
                                 print(f"Target '{target}' not found.")
                 continue

            if target.upper() == "ENTITIES" or command.upper() == "ENTITIES":
                # Try to get entities from Simulation target
                 targets = get_command_list()
                 sim_target = None
                 for t in targets:
                     if "simulation" in t.lower():
                         sim_target = t
                         break
                 
                 if sim_target:
                     result = send_command(sim_target, "LIST_ENTITIES", {})
                     print(f"\nAvailable Entities to Spawn:")
                     # Result will be a string like "Success: ['Rover', ...]" or similar from send_command
                     print(f"  {result}")
                 else:
                     print("Could not find Simulation target to list entities.")
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
