/// A built-in function visible to the LLM.
pub struct BuiltinFunction {
    pub name: &'static str,
    pub summary: &'static str,
    pub annotation: &'static str,
    pub io_only: bool,
}

/// Description for the luau API entry in `list_apis`.
pub const LUAU_DESCRIPTION: &str = "Built-in Luau runtime globals: I/O, JSON, logging. Standard Lua libraries (string, table, math) are also available.";

static BUILTINS: &[BuiltinFunction] = &[
    BuiltinFunction {
        name: "io.open",
        summary: "Open a file for reading, writing, or appending",
        annotation: "\
-- Open a file for reading, writing, or appending.
-- Paths are relative to the I/O directory. Path traversal ('..') is rejected.
-- Modes: \"r\" (default), \"w\", \"a\", \"rb\", \"wb\", \"ab\".
-- Returns a file handle on success, or raises an error.
--
-- File handle methods:
--   handle:read(fmt?) -- \"*a\" (all), \"*l\" (line, default), \"*n\" (number)
--   handle:write(data...) -- returns handle for chaining
--   handle:close() -- returns true
--   handle:seek(whence?, offset?) -- \"set\", \"cur\", \"end\"
--   handle:flush() -- flush write buffer
--   handle:lines() -- line iterator
function io.open(path: string, mode: string?): file_handle end",
        io_only: true,
    },
    BuiltinFunction {
        name: "io.lines",
        summary: "Iterate over lines in a file",
        annotation: "\
-- Iterate over lines in a file. Auto-closes at EOF.
-- Paths are relative to the I/O directory.
--
-- Usage: for line in io.lines(\"data.csv\") do ... end
function io.lines(path: string): () -> string? end",
        io_only: true,
    },
    BuiltinFunction {
        name: "io.list",
        summary: "List directory entries",
        annotation: "\
-- List file and directory names in a directory.
-- Paths are relative to the I/O directory. Defaults to the root I/O directory.
-- Returns an array of entry names (not full paths). Does not recurse.
function io.list(path: string?): {string} end",
        io_only: true,
    },
    BuiltinFunction {
        name: "io.type",
        summary: "Check if a value is a file handle",
        annotation: "\
-- Check if a value is a file handle.
-- Returns \"file\" for an open handle, \"closed file\" for a closed handle, or nil.
function io.type(obj: any): string? end",
        io_only: true,
    },
    BuiltinFunction {
        name: "json.encode",
        summary: "Serialize a Lua value to a JSON string",
        annotation: "\
-- Serialize a Lua value (table, string, number, boolean, nil) to a JSON string.
function json.encode(value: any): string end",
        io_only: false,
    },
    BuiltinFunction {
        name: "json.decode",
        summary: "Parse a JSON string into a Lua value",
        annotation: "\
-- Parse a JSON string into a Lua value.
-- Returns tables for objects/arrays, strings, numbers, booleans, or nil.
function json.decode(str: string): any end",
        io_only: false,
    },
    BuiltinFunction {
        name: "print",
        summary: "Log output (captured in response, not written to stdout)",
        annotation: "\
-- Log output. Arguments are converted to strings and joined with tabs.
-- Output is captured and returned in the 'logs' array of the response.
-- Not written to stdout.
function print(...: any) end",
        io_only: false,
    },
    BuiltinFunction {
        name: "os.remove",
        summary: "Delete a file in the I/O directory",
        annotation: "\
-- Delete a file. Paths are relative to the I/O directory.
-- Cannot delete directories. Raises an error on failure.
function os.remove(path: string): true end",
        io_only: true,
    },
    BuiltinFunction {
        name: "os.clock",
        summary: "Wall-clock time in seconds",
        annotation: "\
-- Returns the wall-clock time in seconds (with fractional part).
-- Useful for measuring elapsed time within a script.
function os.clock(): number end",
        io_only: false,
    },
];

/// Returns all built-in functions. Call with `io_enabled` to filter.
pub fn builtin_functions(io_enabled: bool) -> Vec<&'static BuiltinFunction> {
    BUILTINS
        .iter()
        .filter(|f| io_enabled || !f.io_only)
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn test_builtin_functions_with_io() {
        let funcs = builtin_functions(true);
        assert_eq!(funcs.len(), 9);
        assert!(funcs.iter().any(|f| f.name == "io.open"));
        assert!(funcs.iter().any(|f| f.name == "json.encode"));
    }

    #[test]
    fn test_builtin_functions_without_io() {
        let funcs = builtin_functions(false);
        assert_eq!(funcs.len(), 4); // json.encode, json.decode, print, os.clock
        assert!(funcs.iter().all(|f| !f.io_only));
        assert!(!funcs.iter().any(|f| f.name == "io.open"));
    }

    #[test]
    fn test_all_annotations_non_empty() {
        for f in builtin_functions(true) {
            assert!(!f.annotation.is_empty(), "{} has empty annotation", f.name);
            assert!(
                f.annotation.contains("function"),
                "{} annotation missing function keyword",
                f.name
            );
        }
    }
}
