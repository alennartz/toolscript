# Luau Migration Design

## Context

The code-mcp scripting runtime currently uses Lua 5.4 via mlua with a manually constructed sandbox. This design migrates to Luau (Roblox's Lua fork) to gain:

1. **Native sandbox mode** — `lua.sandbox(true)` replaces ~20 lines of manual deny-listing with a single call that makes globals/metatables read-only and creates isolated per-script environments.
2. **Purpose-built interrupt callback** — `set_interrupt()` replaces instruction-count hooks for timeout enforcement. Fires at VM-determined intervals, more reliable than debug hooks.
3. **Native type annotations** — Luau's type syntax (`x: string`, `export type Pet = {...}`) replaces EmmyLua comment-based annotations, improving LLM codegen quality.
4. **Better memory isolation** — Same `set_memory_limit()` API, but Luau's allocator integration is tighter.

Deployment model: single-tenant / self-hosted. Tenant isolation is handled at infra level. The sandbox primarily prevents LLM-generated scripts from escaping (prompt injection, runaway resources).

## Approach

Switch the mlua feature flag from `lua54` to `luau`. This is the same crate — no dependency change, just a different compilation target. Luau is bundled (no system dependency needed).

## Changes by File

### 1. `Cargo.toml`

```toml
# Before:
mlua = { version = "0.10", features = ["lua54", "vendored", "async", "send", "serialize"] }

# After:
mlua = { version = "0.10", features = ["luau", "async", "send", "serialize"] }
```

Drop `vendored` — the `luau` feature bundles its own source.

### 2. `src/runtime/sandbox.rs` — Sandbox Rewrite

**Remove:**
- `Lua::new_with(StdLib::STRING | StdLib::TABLE | StdLib::MATH, LuaOptions::default())`
- Manual deny-listing of 13 globals (`io`, `os`, `loadfile`, `dofile`, `require`, `debug`, `load`, `package`, `rawget`, `rawset`, `rawequal`, `rawlen`, `collectgarbage`)
- `string.dump` blocking
- `StdLib`, `LuaOptions` imports

**Add:**
- `Lua::new()` — creates Luau VM
- `lua.sandbox(true)` — enables native sandbox:
  - All libraries/metatables become read-only
  - Globals become read-only with safeenv activated
  - Each script gets isolated local environment (writes go local, reads proxy to globals)
  - `collectgarbage` restricted to `"count"` mode only
  - `string.dump` doesn't exist in Luau
  - `io`, `os.execute`, `loadfile`, `dofile`, `package`, `debug` — not part of Luau stdlib

**Keep unchanged:**
- `print()` override (captures log output)
- `json.encode()`/`json.decode()` (Rust-backed via serde)
- Empty `sdk` table (populated by registry)
- Memory limit via `set_memory_limit()`
- `format_lua_value` helper (remove `Value::Integer` branch since Luau only has Number)

### 3. `src/runtime/executor.rs` — Interrupt-based Timeout

**Replace** `set_hook(HookTriggers::every_nth_instruction(1000), ...)` **with** `set_interrupt(...)`:

```rust
sandbox.lua().set_interrupt(move |_lua| {
    if Instant::now() >= deadline {
        Err(mlua::Error::external(anyhow::anyhow!("script execution timed out")))
    } else {
        Ok(VmState::Continue)
    }
});
```

**Remove:**
- `HookTriggers` import (Lua 5.x only)
- `sandbox.lua().remove_hook()` call (interrupt is cleaned up with Lua instance)

**Update `lua_value_to_json`:**
- Remove `Value::Integer` branch (dead code in Luau)
- In `Value::Number` branch, detect whole numbers and serialize as JSON integers:
  ```rust
  Value::Number(n) => {
      if n.fract() == 0.0 && (i64::MIN as f64..=i64::MAX as f64).contains(&n) {
          Ok(serde_json::json!(n as i64))
      } else {
          Ok(serde_json::json!(n))
      }
  }
  ```

### 4. `src/runtime/registry.rs` — Integer Handling

**Update `lua_value_to_string`:**
- Remove `Value::Integer` branch
- In `Value::Number` branch, format whole numbers without decimal:
  ```rust
  Value::Number(n) => {
      if n.fract() == 0.0 { format!("{}", n as i64) } else { n.to_string() }
  }
  ```

**Add type-informed integer rounding** for params with `ParamType::Integer`:
- When the manifest declares a parameter as integer and the Lua value is a number with fractional part, round to nearest integer before encoding as URL parameter.
- This handles floating-point arithmetic drift (e.g., `total / page_size` producing `9.999...`).

### 5. `src/codegen/annotations.rs` — Full Rewrite

Rewrite rendering functions to emit Luau native type syntax instead of EmmyLua comments.

**Schema annotations:**
```
--- @class Pet              →  export type Pet = {
--- @field id string        →      id: string,
--- @field name? string     →      name: string?,
--- @field tags? string[]   →      tags: {string}?,
```

**Function annotations:**
```
--- @param pet_id string    →  function sdk.get_pet(pet_id: string): Pet end
--- @return Pet             →  (types inline in signature)
function sdk.get_pet(pet_id) end
```

**Type mappings:**

| EmmyLua | Luau |
|---------|------|
| `--- @class Pet` | `export type Pet = { ... }` |
| `--- @field name string` | `name: string,` |
| `--- @field name? string` | `name: string?,` |
| `string[]` | `{string}` |
| `"a"\|"b"\|"c"` | `("a" \| "b" \| "c")` |
| `integer` | `number` |
| `--- @param x string Desc` | `-- @param x - Desc` + `x: string` in signature |
| `--- @return Pet` | `: Pet` return type in signature |
| `--- @deprecated` | `-- @deprecated` |

**File extensions:** `.lua` → `.luau` (including `_meta.luau`)

### 6. `src/codegen/generate.rs`

Update file extension from `.lua` to `.luau` in generated output paths.

### 7. `src/server/tools.rs`

Update any hardcoded `.lua` references to `.luau` in annotation file lookups.

## Behavioral Differences

| Lua 5.4 | Luau |
|---------|------|
| Distinct integer/float types | All numbers are f64 |
| `goto` statement | Not available |
| `string.dump` exists (we blocked it) | Doesn't exist |
| `io`, `os`, `debug` in stdlib (we blocked them) | Not in stdlib |
| Debug hooks for timeout | Native interrupt callback |
| Manual sandbox (deny-list) | Native `sandbox(true)` (allow-list) |
| `pairs()`/`ipairs()` required | Generalized `for k, v in t do` also works |
| No `continue` | `continue` keyword available |
| No compound assignment | `+=`, `-=`, etc. available |

For this project's use case (short API orchestration scripts), the syntax overlap is ~95%. Scripts calling SDK functions, handling JSON, and using basic control flow work identically in both.

## Not in Scope

- Process-level isolation (can be added later if deployment model changes to multi-tenant)
- Luau type checking at compile time (would require running `luau-analyze`; not worth the complexity now)
- `require` module system (scripts are self-contained)
