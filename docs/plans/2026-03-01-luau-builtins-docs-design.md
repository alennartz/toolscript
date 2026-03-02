# Luau Built-in Globals Documentation

**Date:** 2026-03-01
**Status:** Approved

## Overview

Expose built-in Luau globals as a virtual `luau` API so the LLM discovers
them through the same `list_apis` → `list_functions` → `get_function_docs` →
`sdk://luau/*` flow it uses for OpenAPI and MCP entries.

## What's Documented in Detail (9 functions)

| Function       | Summary                                              |
|----------------|------------------------------------------------------|
| `io.open`      | Open a file for reading, writing, or appending       |
| `io.lines`     | Iterate over lines in a file                         |
| `io.list`      | List directory entries                               |
| `io.type`      | Check if a value is a file handle                    |
| `json.encode`  | Serialize a Lua value to a JSON string               |
| `json.decode`  | Parse a JSON string into a Lua value                 |
| `print`        | Log output (captured in response, not stdout)        |
| `os.remove`    | Delete a file in the I/O directory                   |
| `os.clock`     | Wall-clock time in seconds                           |

Each gets a full Luau type annotation via `get_function_docs`.

`io.*` and `os.remove` entries are only included when I/O is enabled.

## What's Listed but Not Documented in Detail

The `sdk://luau/overview` resource and the `luau` entry in `list_apis` include:

> Standard Lua libraries are also available: `string`, `table`, `math`,
> `os.clock()`, `os.date()`, `os.difftime()`, `os.time()`. These follow
> standard Lua 5.1 behavior.

## Discovery Paths

- `list_apis` — includes `luau` entry
- `list_functions(api: "luau")` — lists documented globals
- `get_function_docs("io.open")` — full type annotation
- `search_docs("file")` — matches against global docs
- `sdk://luau/overview` — description + available stdlib note
- `sdk://luau/functions` — all documented function signatures
- `sdk://luau/functions/io.open` — individual function docs

## execute_script Description Update

Add to the tool description:

> Only a subset of Lua globals are available in the sandbox. Use
> `list_functions(api: "luau")` or browse `sdk://luau/functions` to see
> built-in functions and their signatures.

## Implementation

Hardcoded static data in Rust. A new module defines the function metadata
and type annotations. They're injected into the same data structures that
`list_apis`, `list_functions`, `get_function_docs`, `search_docs`, and
resources already iterate over.
