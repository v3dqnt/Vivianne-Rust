//! The two tools' definitions, as the model sees them.

use serde_json::Value;

pub fn debug() -> Value {
        serde_json::json!({
            "name": "debug",
            "description": "Look at a program being debugged (started with debug_run launch or attach). Changes nothing. status says where it is; stack_trace, scopes and variables read it while it is stopped; output is what it printed; breakpoints lists them; threads lists its threads; exception_info says why it stopped on an exception; modules, loaded_sources, disassemble and read_memory look at the program itself; completions suggests what to type in an expression.",
            "parameters": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["status", "sessions", "output", "breakpoints", "threads", "stack_trace", "scopes", "variables", "exception_info", "modules", "loaded_sources", "disassemble", "read_memory", "completions"], "default": "status" },
                    "levels": { "type": "number", "description": "stack_trace: how many frames (default 20, at most 100)" },
                    "frame_id": { "type": "number", "description": "scopes, completions: a frame id from stack_trace (default: the top frame)" },
                    "variable_ref": { "type": "number", "description": "variables: a variable_ref from scopes or from another variable" },
                    "start": { "type": "number", "description": "variables: skip this many" },
                    "count": { "type": "number", "description": "variables: how many (default 100, at most 200); read_memory: how many bytes (at most 4096)" },
                    "start_module": { "type": "number", "description": "modules: skip this many" },
                    "module_count": { "type": "number", "description": "modules: how many (at most 100)" },
                    "memory_reference": { "type": "string", "description": "read_memory, disassemble: the address or reference (from a variable, or disassemble); disassemble defaults to the current instruction" },
                    "offset": { "type": "number", "description": "read_memory, disassemble: bytes from the reference" },
                    "instruction_offset": { "type": "number", "description": "disassemble: instructions from the reference (negative for before)" },
                    "instruction_count": { "type": "number", "description": "disassemble: how many (default 20, at most 200)" },
                    "resolve_symbols": { "type": "boolean", "description": "disassemble: name the symbols (default true)" },
                    "text": { "type": "string", "description": "completions: what has been typed so far" },
                    "column": { "type": "number", "description": "completions: where the cursor is in text (1-based, default the end)" }
                }
            }
        })
}

pub fn debug_run() -> Value {
        serde_json::json!({
            "name": "debug_run",
            "description": "Debug a program: launch it under a debug adapter (debugpy for Python, lldb-dap, gdb or codelldb for Rust and C/C++, netcoredbg for .NET, delve for Go, js-debug for JavaScript) or attach to a running one, set breakpoints (conditions, hit counts, log points, functions, exceptions, instructions, watched data), run and step, evaluate expressions, change variables or memory, end it. The program stops at its start, so set breakpoints, then continue. A wait that runs out says the program is still running. Use it when reading the code does not explain a failure; read the result with debug. A launch or attach asks the user first and shows the command it starts; anything the adapter itself asks to run asks again.",
            "parameters": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["launch", "attach", "set_breakpoint", "remove_breakpoint", "exception_breakpoints", "set_instruction_breakpoint", "remove_instruction_breakpoint", "data_breakpoint_info", "set_data_breakpoint", "remove_data_breakpoint", "continue", "step_over", "step_in", "step_out", "pause", "restart", "goto", "evaluate", "set_variable", "write_memory", "custom_request", "terminate"] },
                    "program": { "type": "string", "description": "launch: the file to run (a script, a built binary, or for delve a Go package directory)" },
                    "args": { "type": "array", "items": { "type": "string" }, "description": "launch: the program's arguments" },
                    "cwd": { "type": "string", "description": "launch, attach: the working directory (default: the workspace)" },
                    "adapter": { "type": "string", "enum": ["debugpy", "lldb-dap", "gdb", "codelldb", "netcoredbg", "delve", "js-debug"], "description": "launch, attach: only to override the choice made from the file type (a dap.json may define others)" },
                    "pid": { "type": "number", "description": "attach: the process to attach to" },
                    "port": { "type": "number", "description": "attach: a port the program listens on for a debugger (debugpy)" },
                    "host": { "type": "string", "description": "attach: the host for port (default 127.0.0.1)" },
                    "breakpoints": { "type": "array", "items": { "type": "object", "properties": { "file": { "type": "string" }, "line": { "type": "number" }, "condition": { "type": "string" } }, "required": ["file", "line"] }, "description": "launch, attach: breakpoints set before the program runs" },
                    "file": { "type": "string", "description": "set_breakpoint, remove_breakpoint, goto: the file" },
                    "line": { "type": "number", "description": "set_breakpoint, remove_breakpoint, goto: 1-based line" },
                    "function": { "type": "string", "description": "set_breakpoint, remove_breakpoint: a function name instead of file and line" },
                    "condition": { "type": "string", "description": "breakpoints: stop only when this is true" },
                    "hit_condition": { "type": "string", "description": "breakpoints: stop only on this hit count (for example 5, or > 3)" },
                    "log_message": { "type": "string", "description": "set_breakpoint: print this instead of stopping ({expression} is evaluated)" },
                    "filters": { "type": "array", "items": { "type": "string" }, "description": "exception_breakpoints: the adapter's filters (for example raised, uncaught); an empty list clears them" },
                    "instruction_reference": { "type": "string", "description": "instruction breakpoints: the address or reference" },
                    "offset": { "type": "number", "description": "instruction breakpoints, write_memory: bytes from the reference" },
                    "name": { "type": "string", "description": "set_variable, data_breakpoint_info: the variable" },
                    "variable_ref": { "type": "number", "description": "set_variable, data_breakpoint_info: the scope or variable that holds it" },
                    "data_id": { "type": "string", "description": "set_data_breakpoint, remove_data_breakpoint: from data_breakpoint_info" },
                    "access_type": { "type": "string", "enum": ["read", "write", "readWrite"], "description": "set_data_breakpoint: what triggers it" },
                    "expression": { "type": "string", "description": "evaluate: runs in the program, in the stopped frame" },
                    "frame_id": { "type": "number", "description": "evaluate, data_breakpoint_info: a frame id from debug stack_trace (default: the top frame)" },
                    "context": { "type": "string", "description": "evaluate: repl (default), watch or hover" },
                    "value": { "type": "string", "description": "set_variable: the new value, as an expression in the program's language" },
                    "memory_reference": { "type": "string", "description": "write_memory: where" },
                    "data": { "type": "string", "description": "write_memory: the bytes, base64 (at most 4096)" },
                    "allow_partial": { "type": "boolean", "description": "write_memory: accept a partial write" },
                    "command": { "type": "string", "description": "custom_request: a debug adapter request that has no action here" },
                    "arguments": { "type": "object", "description": "custom_request: its arguments" },
                    "granularity": { "type": "string", "enum": ["statement", "line", "instruction"], "description": "steps: how far one step goes" },
                    "single_thread": { "type": "boolean", "description": "steps and continue: move only the thread that stopped" },
                    "timeout": { "type": "number", "description": "continue, steps, restart, goto: seconds to wait for the next stop (5 to 300, default 30)" }
                },
                "required": ["action"]
            }
        })
}
