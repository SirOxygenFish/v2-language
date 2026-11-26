--[[
lua_adapter.lua: a VEP/1 bridged engine adapter for Lua 5.4 and 5.5, standard library only.

    [engines.luab]                    # in v2.toml; `lua` itself is V2's built-in run engine
    kind    = "bridged"
    command = ["lua", "examples/lua_adapter.lua"]

The examples below write the tag as `lua`; read it as the tag you chose.

Caps: "modules" and "reverse" (no "handles", no "kwargs"). Written from the spec alone (DOCS.md 4, 6
and 13; INTERNALS.md 9-14 and 17).

How V2 values look in Lua (decisions, INTERNALS 10):
- null is nil where a value stands alone (block args, call arguments, results, a block's value).
  Inside lists and dicts it is the sentinel V2.null, so a list keeps its length and a dict its keys.
  nil and V2.null both go out as null. (V2.null is truthy in Lua: compare with V2.null.)
- int <-> Lua integer, float <-> Lua float (math.type keeps the kind: 1.0 stays a float; -0.0,
  nan and inf survive). Ints beyond 2^53 use the int tag both ways.
- str <-> Lua string. A Lua string that is not valid UTF-8 is a MarshalError when sent as a value
  (Lua strings are bytes; guessing "bytes" from the content would change a value's type by its
  data): wrap binary data in V2.bytes(s). In text (output, error messages, traces) bad bytes
  become U+FFFD instead.
- bytes <-> a V2.bytes object: b.data is the raw string, #b its length, tostring(b) the raw bytes.
- list <-> a table marked V2.list; dict <-> a table marked V2.dict (its key order is kept for the
  trip back). An unmarked table goes out as a list when its keys are exactly 1..n, as a dict when
  all keys are strings, and an empty unmarked table {} as an empty LIST. For an empty dict use
  V2.dict() (V2.dict(t) / V2.list(t) mark an existing table). Other key mixes, cycles, more than
  256 levels, and tables with a metatable of their own (objects) are MarshalErrors: without the
  handles cap nothing can go out as a handle.
- A dict with a "$v2" key travels in the dict tag, both ways.
- V2 functions (and other languages' functions) arrive as real Lua functions; calling one calls
  back into V2. A Lua function cannot be sent to V2 as a value (no handles): @export it.
- A V2 error value arrives as a V2Error object (V2.iserror(e)); raised, it goes back unchanged.

Blocks:
- Each unit runs in its own environment table whose __index is the real globals (fresh for an
  action run, kept for a definition). @export names must be globals of the block, not locals.
- A block's value: `return (<source>)` is tried first, like Lua's REPL, else the chunk runs and
  its first return value is the value (`local x = 2  return x * 2`). Statement blocks use the
  same rule, so `@lua { f() }` and `@lua { 1 + 2 }` both run.
- Block args and import aliases must be Lua names (not keywords); V2 and _ENV are reserved.
- @import { f } from lua.mod is require("mod"), with the .v2 file's folder and the project root
  first on package.path. The whole-module form (@import lua.mod as m) needs handles: ImportError.

Errors (err frames, INTERNALS 9.6):
- error("text") -> ForeignError, foreign_type "error", message without the "file:line:" prefix.
- error({type = "ParseError", message = "bad input", line = 3}) -> foreign_type "ParseError"
  (else the metatable's __name, else "error"), message from .message (else tostring), and the
  table's other simple fields in e.data. Other values: tostring, or "(error object is a X value)".
- A syntax error -> ForeignError, foreign_type "SyntaxError", at the .v2 line.
- Trace frames: the .v2 path is the chunk name and line - 1 newlines go in front of the source
  (Lua has no line offset), so Lua's own line numbers are .v2 lines; col is always null.

Stdio (INTERNALS 9.9): Lua has no dup2, so the language level is wrapped instead: print,
io.write, io.stdout and io.stderr send out frames; io.read, io.lines(), io.stdin and debug.debug
raise; os.exit raises (e.data.exit_code); os.execute and io.popen children get stdin from the null
device and stdout on stderr. Not covered (the remaining gap): io.open("/dev/stdout"), C modules
writing fd 1, and a child's own redirections; those corrupt the protocol stream.
Releases: V2 function stubs are collected by Lua's GC; their release goes out before the next
frame, and the adapter runs a full collection after a request once 64 stubs were made.
]]

local REAL = {
  stdin = io.stdin, stdout = io.stdout, stderr = io.stderr,
  exit = os.exit, execute = os.execute, popen = io.popen, lines = io.lines, iotype = io.type,
  open = io.open,
}
local G = _G
local LIMIT = 64 * 1024 * 1024
local MAXSAFE = 9007199254740991
local SEP = package.config:sub(1, 1)
local WINDOWS = SEP == "\\"
local ADAPTER_SOURCE = debug.getinfo(1, "S").source
local HINT_BIG = "write it to a file and pass the file's path, or send it in parts"
local STDIN_MSG = "blocks cannot read standard input: V2's input() owns it; pass the data in as a block arg"

local STACK = {}        -- "request" / "waiting" entries (INTERNALS 14.2)
local UNITS = {}        -- wire unit id -> {env, kinds} (definitions) or {mod, name, kinds} (modules)
local PENDING = {}      -- unit -> weak env -> {alias -> {stub, name}}: same-session stubs to replace
local KNOWN_SRC = {}    -- short_src of a block chunk -> .v2 path
local RELQ = {}         -- callable ids whose stubs died
local OUT_SEGS, OUT_N, OUT_SIZE = {}, 0, 0
local OUT_ID = 0
local TAG = "lua"
local PROJECT_ROOT = nil
local BYE = false
local STUBS_MADE = 0

------------------------------------------------------------------------------------------------
-- Sentinels, markers, V2Error

local NULL = setmetatable({}, {
  __name = "v2.null", __tostring = function() return "null" end, __metatable = "v2.null",
  __newindex = function() error("V2.null can't be changed", 2) end,
})
local LIST_MT = {__v2 = "list", __name = "v2.list"}
local BYTES_MT = {__name = "v2.bytes"}
BYTES_MT.__tostring = function(b) return rawget(b, "data") end
BYTES_MT.__len = function(b) return #rawget(b, "data") end
BYTES_MT.__eq = function(a, b)
  return getmetatable(a) == BYTES_MT and getmetatable(b) == BYTES_MT and rawget(a, "data") == rawget(b, "data")
end
local V2ERR_MT = {__name = "V2Error"}
V2ERR_MT.__tostring = function(e)
  local t = rawget(e, "type")
  local m = tostring(rawget(e, "message"))
  if t == nil or t == "Error" then return m end
  return t .. ": " .. m
end
local SYNTAX_MT = {}
local RAWOBJ, RAWARR, RAW_NULL = {}, {}, {}

local function v2err(kind, message, hint)
  return setmetatable({type = kind, message = message, hint = hint}, V2ERR_MT)
end
local function pfail(msg) error(v2err("ProtocolError", msg), 0) end

local STUB_INFO = setmetatable({}, {__mode = "k"})   -- stub function -> {kind, cell|route, name}
local STUBS_BY_ID = setmetatable({}, {__mode = "v"}) -- callable id -> stub function
local ANCHOR_MT = {__gc = function(a)
  local c = a.cell
  for _ = 1, c.n do RELQ[#RELQ + 1] = c.id end
end}

------------------------------------------------------------------------------------------------
-- Text helpers

local function bad_length(s, p)
  -- bytes one U+FFFD replaces at p: a lead byte and the continuation bytes of its cut sequence
  local b = s:byte(p)
  local need, lo, hi = 0, 0x80, 0xBF
  if b >= 0xC2 and b <= 0xDF then need = 1
  elseif b >= 0xE0 and b <= 0xEF then need = 2
    if b == 0xE0 then lo = 0xA0 elseif b == 0xED then hi = 0x9F end
  elseif b >= 0xF0 and b <= 0xF4 then need = 3
    if b == 0xF0 then lo = 0x90 elseif b == 0xF4 then hi = 0x8F end
  end
  local k = 0
  while k < need do
    local c = s:byte(p + 1 + k)
    if not c or c < lo or c > hi then break end
    lo, hi = 0x80, 0xBF
    k = k + 1
  end
  return k < need and k + 1 or 1
end

local function well_formed(s)
  if utf8.len(s) then return s end
  local parts, i, len = {}, 1, #s
  while i <= len do
    local ok, bad = utf8.len(s, i)
    if ok then parts[#parts + 1] = s:sub(i); break end
    parts[#parts + 1] = s:sub(i, bad - 1)
    parts[#parts + 1] = "\239\191\189"
    i = bad + bad_length(s, bad)
  end
  return table.concat(parts)
end

local ESC_OUT = {['"'] = '\\"', ['\\'] = '\\\\', ['\n'] = '\\n', ['\r'] = '\\r', ['\t'] = '\\t',
                 ['\b'] = '\\b', ['\f'] = '\\f'}
for i = 0, 31 do
  local c = string.char(i)
  if not ESC_OUT[c] then ESC_OUT[c] = string.format("\\u%04x", i) end
end
local function quote(s)
  return '"' .. s:gsub('[%c"\\]', ESC_OUT) .. '"'
end
local function qtext(s) return quote(well_formed(tostring(s))) end

local function fmt_float(x)
  if x ~= x then return '{"$v2":"float","v":"nan"}' end
  if x == math.huge then return '{"$v2":"float","v":"inf"}' end
  if x == -math.huge then return '{"$v2":"float","v":"-inf"}' end
  local s
  for p = 1, 17 do
    s = string.format("%." .. p .. "g", x):gsub(",", ".")
    if tonumber(s) == x then break end
  end
  if not s:find("[%.eEn]") then s = s .. ".0" end
  return s
end

local B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
local B64E, B64D = {}, {}
for i = 1, 64 do B64E[i - 1] = B64:sub(i, i); B64D[B64:byte(i)] = i - 1 end
local function b64enc(s)
  local out, n, len = {}, 0, #s
  for i = 1, len - 2, 3 do
    local a, b, c = s:byte(i, i + 2)
    local v = (a << 16) | (b << 8) | c
    n = n + 1
    out[n] = B64E[v >> 18] .. B64E[(v >> 12) & 63] .. B64E[(v >> 6) & 63] .. B64E[v & 63]
  end
  local rem = len % 3
  if rem == 1 then
    local v = s:byte(len) << 16
    out[n + 1] = B64E[v >> 18] .. B64E[(v >> 12) & 63] .. "=="
  elseif rem == 2 then
    local a, b = s:byte(len - 1, len)
    local v = (a << 16) | (b << 8)
    out[n + 1] = B64E[v >> 18] .. B64E[(v >> 12) & 63] .. B64E[(v >> 6) & 63] .. "="
  end
  return table.concat(out)
end
local function b64dec(t)
  local len = #t
  if len % 4 ~= 0 then return nil end
  local out, n = {}, 0
  for i = 1, len, 4 do
    local c1, c2, c3, c4 = t:byte(i, i + 3)
    local a, b = B64D[c1], B64D[c2]
    if not a or not b then return nil end
    if c4 == 61 then
      if i + 3 ~= len then return nil end
      if c3 == 61 then
        if b & 15 ~= 0 then return nil end
        out[n + 1] = string.char((a << 2) | (b >> 4))
      else
        local c = B64D[c3]
        if not c or c & 3 ~= 0 then return nil end
        out[n + 1] = string.char((a << 2) | (b >> 4), ((b & 15) << 4) | (c >> 2))
      end
      n = n + 1
    else
      local c, d = B64D[c3], B64D[c4]
      if not c or not d then return nil end
      local v = (a << 18) | (b << 12) | (c << 6) | d
      n = n + 1
      out[n] = string.char(v >> 16, (v >> 8) & 255, v & 255)
    end
  end
  return table.concat(out)
end

------------------------------------------------------------------------------------------------
-- JSON parsing into raw trees (objects keep their key order; decoded top-down later, 10.1)

local ESC_IN = {[34] = '"', [92] = '\\', [47] = '/', [98] = '\b', [102] = '\f', [110] = '\n',
                [114] = '\r', [116] = '\t'}

local function json_parse(s)
  local pos, len = 1, #s
  local function perr(msg) error("invalid JSON: " .. msg .. " at byte " .. pos, 0) end
  local function skip() pos = s:find("[^ \t\r\n]", pos) or len + 1 end
  local function parse_string()
    local i, parts, np = pos + 1, nil, 0
    while true do
      local j = s:find('["\\\0-\31]', i)
      if not j then perr("unterminated string") end
      local c = s:byte(j)
      if c == 34 then
        pos = j + 1
        if not parts then return s:sub(i, j - 1) end
        np = np + 1; parts[np] = s:sub(i, j - 1)
        return table.concat(parts)
      elseif c == 92 then
        parts = parts or {}
        np = np + 1; parts[np] = s:sub(i, j - 1)
        local e = s:byte(j + 1)
        if e == 117 then
          local hex = s:match("^%x%x%x%x", j + 2)
          if not hex then pos = j; perr("bad \\u escape") end
          local cp, nexti = tonumber(hex, 16), j + 6
          if cp >= 0xD800 and cp <= 0xDBFF then
            local lo = s:match("^\\u(%x%x%x%x)", nexti)
            lo = lo and tonumber(lo, 16)
            if lo and lo >= 0xDC00 and lo <= 0xDFFF then
              cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00)
              nexti = nexti + 6
            else
              cp = 0xFFFD
            end
          elseif cp >= 0xDC00 and cp <= 0xDFFF then
            cp = 0xFFFD
          end
          np = np + 1; parts[np] = utf8.char(cp)
          i = nexti
        else
          local m = ESC_IN[e]
          if not m then pos = j; perr("bad escape") end
          np = np + 1; parts[np] = m
          i = j + 2
        end
      else
        pos = j; perr("control character in a string")
      end
    end
  end
  local parse_value
  local function parse_number()
    local st = pos
    local _, e = s:find("^-?%d+", pos)
    if not e then perr("unexpected character") end
    if s:find("^-?0%d", st) then perr("leading zero") end
    local isfloat = false
    local _, e2 = s:find("^%.%d+", e + 1)
    if e2 then e = e2; isfloat = true end
    local _, e3 = s:find("^[eE][-+]?%d+", e + 1)
    if e3 then e = e3; isfloat = true end
    local text = s:sub(st, e)
    pos = e + 1
    if text == "-0" then return -0.0 end
    return tonumber(text)   -- a float with . or e; an integer when it fits i64, else the nearest float (10.2)
  end
  parse_value = function(depth)
    if depth > 1100 then perr("nesting too deep") end
    skip()
    local c = s:byte(pos)
    if c == 123 then
      pos = pos + 1
      local keys, map = {}, {}
      local obj = setmetatable({keys = keys, map = map}, RAWOBJ)
      skip()
      if s:byte(pos) == 125 then pos = pos + 1; return obj end
      while true do
        skip()
        if s:byte(pos) ~= 34 then perr("expected a key") end
        local k = parse_string()
        skip()
        if s:byte(pos) ~= 58 then perr("expected ':'") end
        pos = pos + 1
        local v = parse_value(depth + 1)
        if map[k] == nil then keys[#keys + 1] = k end
        map[k] = v
        skip()
        local d = s:byte(pos)
        pos = pos + 1
        if d == 125 then return obj end
        if d ~= 44 then pos = pos - 1; perr("expected ',' or '}'") end
      end
    elseif c == 91 then
      pos = pos + 1
      local arr = setmetatable({n = 0}, RAWARR)
      skip()
      if s:byte(pos) == 93 then pos = pos + 1; return arr end
      while true do
        local v = parse_value(depth + 1)
        arr.n = arr.n + 1
        arr[arr.n] = v
        skip()
        local d = s:byte(pos)
        pos = pos + 1
        if d == 93 then return arr end
        if d ~= 44 then pos = pos - 1; perr("expected ',' or ']'") end
      end
    elseif c == 34 then
      return parse_string()
    elseif c == 116 then
      if s:sub(pos, pos + 3) ~= "true" then perr("unexpected word") end
      pos = pos + 4; return true
    elseif c == 102 then
      if s:sub(pos, pos + 4) ~= "false" then perr("unexpected word") end
      pos = pos + 5; return false
    elseif c == 110 then
      if s:sub(pos, pos + 3) ~= "null" then perr("unexpected word") end
      pos = pos + 4; return RAW_NULL
    elseif c == nil then
      perr("unexpected end")
    end
    return parse_number()
  end
  local v = parse_value(0)
  skip()
  if pos <= len then perr("trailing text") end
  return v
end

local raw_ser
raw_ser = function(r, out)
  if r == RAW_NULL then out[#out + 1] = "null"
  elseif type(r) == "boolean" then out[#out + 1] = r and "true" or "false"
  elseif type(r) == "number" then
    out[#out + 1] = math.type(r) == "integer" and string.format("%d", r) or fmt_float(r)
  elseif type(r) == "string" then out[#out + 1] = quote(r)
  elseif getmetatable(r) == RAWARR then
    out[#out + 1] = "["
    for i = 1, r.n do
      if i > 1 then out[#out + 1] = "," end
      raw_ser(r[i], out)
    end
    out[#out + 1] = "]"
  else
    out[#out + 1] = "{"
    for i, k in ipairs(r.keys) do
      if i > 1 then out[#out + 1] = "," end
      out[#out + 1] = quote(k) .. ":"
      raw_ser(r.map[k], out)
    end
    out[#out + 1] = "}"
  end
  return out
end
local function raw_json(r) return table.concat(raw_ser(r, {})) end
local function rstr(x) if type(x) == "string" then return x end return nil end

------------------------------------------------------------------------------------------------
-- Encoding Lua values (10.2-10.5)

local function mfail(msg, path, hint)
  if path ~= "" then msg = msg .. " (at " .. path .. ")" end
  error(v2err("MarshalError", msg, hint), 0)
end
local function put(B, s)
  local n = B.n + 1
  B[n] = s; B.n = n
  B.size = B.size + #s
  if B.size > LIMIT then
    error(v2err("MarshalError", string.format("value too large to send to V2 (about %d MiB, limit 64 MiB)",
      B.size // 1048576), HINT_BIG), 0)
  end
end
local function key_text(k)
  if type(k) == "string" then return quote(k) end
  local kind = math.type(k) or type(k)
  return tostring(k) .. ((kind == "integer") and " (an " or " (a ") .. kind .. ")"
end
local function ordered_keys(v, mt)
  local keys, have = {}, {}
  local rec = mt and rawget(mt, "__keys")
  if rec then
    for _, k in ipairs(rec) do
      if rawget(v, k) ~= nil and not have[k] then keys[#keys + 1] = k; have[k] = true end
    end
  end
  local extra = {}
  for k in next, v do if not have[k] then extra[#extra + 1] = k end end
  table.sort(extra)
  for _, k in ipairs(extra) do keys[#keys + 1] = k end
  return keys
end

local enc
local function enc_table(B, v, mt, depth, path, seen)
  if seen[v] then mfail("cannot send a table that contains itself", path) end
  if depth > 256 then mfail("value nested deeper than 256 levels", path) end
  local kind = mt and rawget(mt, "__v2")
  if mt and not kind then
    mfail("a table with a metatable (an object) can't be copied to V2, and this adapter has no handles: send plain tables", path)
  end
  local count, maxk, nonstr, nonint = 0, 0, nil, nil
  for k in next, v do
    count = count + 1
    if math.type(k) == "integer" and k >= 1 then
      if k > maxk then maxk = k end
      if nonstr == nil then nonstr = k end
    elseif type(k) == "string" then
      if nonint == nil then nonint = k end
    else
      if nonint == nil then nonint = k end
      if nonstr == nil then nonstr = k end
    end
  end
  if kind == nil then
    if count == 0 or (nonint == nil and maxk == count) then kind = "list"
    elseif nonstr == nil then kind = "dict"
    else
      mfail("a table with the key " .. key_text(nonstr) .. " and the key " .. key_text(nonint)
        .. " is neither a list (keys 1..n) nor a dict (string keys)", path)
    end
  end
  seen[v] = true
  if kind == "list" then
    if nonint ~= nil then mfail("a V2.list table has the key " .. key_text(nonint), path) end
    put(B, "[")
    for i = 1, maxk do
      if i > 1 then put(B, ",") end
      enc(B, rawget(v, i), depth + 1, path .. "[" .. (i - 1) .. "]", seen)
    end
    put(B, "]")
  else
    if nonstr ~= nil then mfail("dict keys must be strings, got " .. key_text(nonstr), path) end
    local wrap = rawget(v, "$v2") ~= nil
    put(B, wrap and '{"$v2":"dict","v":{' or "{")
    for i, k in ipairs(ordered_keys(v, mt)) do
      if not utf8.len(k) then mfail("a dict key is not valid UTF-8", path) end
      if i > 1 then put(B, ",") end
      put(B, quote(k) .. ":")
      enc(B, rawget(v, k), depth + 1, path .. "[" .. quote(k) .. "]", seen)
    end
    put(B, wrap and "}}" or "}")
  end
  seen[v] = nil
end

enc = function(B, v, depth, path, seen)
  if v == nil or v == NULL then put(B, "null"); return end
  local tv = type(v)
  if tv == "boolean" then put(B, v and "true" or "false")
  elseif tv == "number" then
    if math.type(v) == "integer" then
      if v >= -MAXSAFE and v <= MAXSAFE then put(B, string.format("%d", v))
      else put(B, '{"$v2":"int","v":"' .. string.format("%d", v) .. '"}') end
    else
      put(B, fmt_float(v))
    end
  elseif tv == "string" then
    local ok, at = utf8.len(v)
    if not ok then
      mfail(string.format("string is not valid UTF-8 (byte 0x%02x at offset %d)", v:byte(at), at - 1), path,
        "send binary data as bytes, V2.bytes(s), or make it text in the block")
    end
    put(B, quote(v))
  elseif tv == "table" then
    local mt = getmetatable(v)
    if mt == BYTES_MT then
      put(B, '{"$v2":"bytes","b64":"' .. b64enc(rawget(v, "data")) .. '"}')
    elseif mt == V2ERR_MT then
      if rawget(v, "_raw") then put(B, raw_json(v._raw))
      else
        put(B, '{"$v2":"error","type":' .. qtext(v.type or "Error") .. ',"message":' .. qtext(v.message or "")
          .. ',"data":null,"lang":' .. quote(TAG) .. ',"foreign_type":null,"file":null,"line":null,"col":null,"trace":[],"cause":null}')
      end
    else
      enc_table(B, v, mt, depth, path, seen)
    end
  elseif tv == "function" then
    local info = STUB_INFO[v]
    if info and info.kind == "fn" then put(B, '{"$v2":"fn","id":' .. info.cell.id .. '}')
    elseif info and info.route then put(B, '{"$v2":"fn","route":' .. info.route .. '}')
    else
      mfail("a Lua function can't be sent to V2 as a value: this adapter has no handles", path,
        "@export the function from a block and @import it in V2")
    end
  else
    mfail("a Lua " .. tv .. " can't be sent to V2: this adapter has no handles", path)
  end
end

local function encode(v, root)
  local B = {n = 0, size = 0}
  enc(B, v, 1, root or "", {})
  return table.concat(B, "", 1, B.n)
end

------------------------------------------------------------------------------------------------
-- Decoding raw trees into Lua values

local host_call   -- forward
local function to_lua(v) if v == NULL then return nil end return v end

local function make_fn_stub(id, name)
  local f = STUBS_BY_ID[id]
  if f then
    local cell = STUB_INFO[f].cell
    cell.n = cell.n + 1
    return f
  end
  local cell = {id = id, n = 1}
  local anchor = setmetatable({cell = cell}, ANCHOR_MT)
  f = function(...) return host_call('{"fn":' .. anchor.cell.id .. '}', name, false, ...) end
  STUB_INFO[f] = {kind = "fn", cell = cell, name = name}
  STUBS_BY_ID[id] = f
  STUBS_MADE = STUBS_MADE + 1
  return f
end

local dec
local function dec_plain_obj(r, depth)
  local t, keys = {}, {}
  for i, k in ipairs(r.keys) do
    keys[i] = k
    t[k] = dec(r.map[k], depth + 1)
  end
  return setmetatable(t, {__v2 = "dict", __name = "v2.dict", __keys = keys})
end
local unit_target  -- forward

dec = function(r, depth)
  if r == RAW_NULL then return NULL end
  if type(r) ~= "table" then return r end
  if depth > 300 then pfail("value nested too deeply") end
  if getmetatable(r) == RAWARR then
    local t = {}
    for i = 1, r.n do t[i] = dec(r[i], depth + 1) end
    return setmetatable(t, LIST_MT)
  end
  local m = r.map
  local tag = m["$v2"]
  if tag == nil then return dec_plain_obj(r, depth) end
  if tag == "int" then
    local n = type(m.v) == "string" and m.v:find("^-?%d+$") and math.tointeger(tonumber(m.v))
    if not n then pfail("bad int tag") end
    return n
  elseif tag == "float" then
    if m.v == "nan" then return 0.0 / 0.0 elseif m.v == "inf" then return math.huge
    elseif m.v == "-inf" then return -math.huge end
    pfail("bad float tag")
  elseif tag == "bytes" then
    local s = type(m.b64) == "string" and b64dec(m.b64)
    if not s then pfail("bad bytes tag") end
    return setmetatable({data = s}, BYTES_MT)
  elseif tag == "dict" then
    if getmetatable(m.v) ~= RAWOBJ then pfail("bad dict tag") end
    return dec_plain_obj(m.v, depth)
  elseif tag == "fn" then
    if math.type(m.id) == "integer" then return make_fn_stub(m.id, rstr(m.name) or "function") end
    if type(m.unit) == "string" and type(m.name) == "string" then return unit_target(m.unit, m.name) end
    pfail("bad fn tag")
  elseif tag == "error" then
    if type(m.type) ~= "string" or type(m.message) ~= "string" then pfail("bad error tag") end
    local cause = m.cause
    return setmetatable({
      type = m.type, message = m.message, data = to_lua(dec(m.data or RAW_NULL, depth + 1)),
      lang = rstr(m.lang), foreign_type = rstr(m.foreign_type), file = rstr(m.file),
      line = math.type(m.line) == "integer" and m.line or nil, col = math.type(m.col) == "integer" and m.col or nil,
      trace = getmetatable(m.trace) == RAWARR and dec(m.trace, depth + 1) or nil,
      cause = (cause ~= nil and cause ~= RAW_NULL) and dec(cause, depth + 1) or nil,
      hint = rstr(m.hint), _raw = r,
    }, V2ERR_MT)
  elseif tag == "h" then
    pfail("V2 sent a handle, but this adapter has no 'handles' capability")
  end
  pfail("unknown $v2 tag " .. tostring(tag))
end

local function dec_args(raw)
  local args, n = {}, 0
  if getmetatable(raw) == RAWARR then
    n = raw.n
    for i = 1, n do args[i] = to_lua(dec(raw[i], 0)) end
  end
  return args, n
end

------------------------------------------------------------------------------------------------
-- Output (9.9) and frames

local function raw_write(s)
  local ok = REAL.stdout:write(s, "\n")
  if ok then ok = REAL.stdout:flush() end
  if not ok then REAL.exit(0, false) end   -- the host is gone (9.1)
end

local CHUNK = 1024 * 1024
local function incomplete_tail(text)
  -- length of a UTF-8 sequence cut at the end of text (0 when it ends on a whole character)
  local len = #text
  for back = 1, math.min(3, len) do
    local b = text:byte(len - back + 1)
    if b & 0xC0 ~= 0x80 then
      local need = (b >= 0xF0 and 4) or (b >= 0xE0 and 3) or (b >= 0xC0 and 2) or 1
      if need > back then return back end
      return 0
    end
  end
  return 0
end

local function out_flush(forced)
  if OUT_N == 0 then return end
  local segs = OUT_SEGS
  OUT_SEGS, OUT_N, OUT_SIZE = {}, 0, 0
  for i = 1, #segs do
    local seg = segs[i]
    local text = table.concat(seg.parts)
    if not forced and i == #segs then
      local cut = incomplete_tail(text)
      if cut > 0 then
        OUT_SEGS[1] = {stream = seg.stream, parts = {text:sub(-cut)}}
        OUT_N, OUT_SIZE = 1, cut
        text = text:sub(1, -cut - 1)
      end
    end
    text = well_formed(text)
    local pos, len = 1, #text
    while pos <= len do
      local stop = pos + CHUNK - 1
      if stop < len then
        while stop > pos and (text:byte(stop + 1) & 0xC0) == 0x80 do stop = stop - 1 end
      else
        stop = len
      end
      raw_write('{"t":"out","stream":"' .. seg.stream .. '","text":' .. quote(text:sub(pos, stop)) .. '}')
      pos = stop + 1
    end
  end
end

local function out_write(stream, s)
  if s == "" then return end
  local last = OUT_SEGS[#OUT_SEGS]
  if not last or last.stream ~= stream then
    last = {stream = stream, parts = {}}
    OUT_SEGS[#OUT_SEGS + 1] = last
  end
  last.parts[#last.parts + 1] = s
  OUT_N = OUT_N + 1
  OUT_SIZE = OUT_SIZE + #s
  if (s:byte(-1) == 10 or OUT_SIZE >= 65536) and (BYE or STACK[#STACK] == "request") then
    out_flush(false)
  end
end

local function send(f)
  out_flush(true)
  if #RELQ > 0 then
    local q = RELQ
    RELQ = {}
    raw_write('{"t":"release","fn":[' .. table.concat(q, ",") .. ']}')
  end
  raw_write(f)
end

local function bad_input(what, line)
  local shown = line:sub(1, 80):gsub("[%c\128-\255]", function(c) return string.format("\\x%02x", c:byte()) end)
  REAL.stderr:write("lua adapter: " .. what .. " (got: " .. shown .. ")\n")
  REAL.stderr:flush()
  REAL.exit(2, false)
end

------------------------------------------------------------------------------------------------
-- Errors -> err.error (9.6)

local HANDLER_FUNCS = {}

local CALL_NAMES = {}   -- request depth -> the export a host call named (Lua names a function by its caller)

local function collect_frames(level)
  local frames, srcs, stopped, last = {}, {}, false, nil
  while true do
    local info = debug.getinfo(level, "Slnf")
    if not info then break end
    if not stopped and HANDLER_FUNCS[info.func] then
      stopped = true
      if last == level - 1 and frames[#frames].func and CALL_NAMES[#STACK] then
        frames[#frames].func = CALL_NAMES[#STACK]
      end
    end
    if info.what ~= "C" and info.currentline and info.currentline > 0 then
      srcs[info.short_src .. ":" .. info.currentline] = true
      if not stopped and info.source ~= ADAPTER_SOURCE then
        local src, file = info.source, nil
        local c = src:sub(1, 1)
        if c == "@" or c == "=" then file = src:sub(2) end
        if file then
          frames[#frames + 1] = {file = file, line = info.currentline,
                                 func = info.what ~= "main" and (info.name or "?") or nil}
          last = level
        end
      end
    end
    level = level + 1
  end
  return frames, srcs
end

local function on_error(e)
  local frames, srcs = collect_frames(2)
  return {report = true, e = e, frames = frames, srcs = srcs}
end

local function strip_where(e, srcs, frames)
  local src, line, rest = e:match("^(.-):(%d+): (.*)$")
  if not src then return e end
  local file = KNOWN_SRC[src]
  if srcs[src .. ":" .. line] or file or src:find("%.lua$") or src:find("%.v2$") then
    if file then
      local has = false
      for _, f in ipairs(frames) do if f.file == file then has = true; break end end
      if not has then table.insert(frames, 1, {file = file, line = tonumber(line)}) end
    end
    return rest
  end
  return e
end

local function simple_fields(t)
  local names = {}
  for k, v in next, t do
    local tv = type(v)
    if type(k) == "string" and k ~= "type" and k ~= "message" and k ~= "$v2" and utf8.len(k)
      and (tv == "boolean" or (tv == "number" and (math.type(v) == "float" or (v >= -MAXSAFE and v <= MAXSAFE)))
           or (tv == "string" and utf8.len(v))) then
      names[#names + 1] = k
    end
  end
  if #names == 0 then return nil end
  table.sort(names)
  local parts = {}
  for i = 1, math.min(32, #names) do
    local k = names[i]
    parts[i] = quote(k) .. ":" .. encode(rawget(t, k))
  end
  return "{" .. table.concat(parts, ",") .. "}"
end

local function frames_json(frames)
  local parts = {}
  for i, f in ipairs(frames) do
    parts[i] = '{"file":' .. qtext(f.file) .. ',"line":' .. string.format("%d", f.line)
      .. ',"col":null,"func":' .. (f.func and qtext(f.func) or "null") .. '}'
  end
  return "[" .. table.concat(parts, ",") .. "]"
end

local function error_json(rep)
  local e, frames = rep.e, rep.frames or {}
  local etype, msg, ftype, data, hint, v2 = "ForeignError", nil, "error", nil, nil, nil
  local te = type(e)
  local mt = te == "table" and getmetatable(e) or nil
  if mt == SYNTAX_MT then
    ftype, msg = "SyntaxError", e.message
    frames = {{file = e.file, line = e.line or 1}}
  elseif mt == V2ERR_MT then
    msg = tostring(rawget(e, "message"))
    if rawget(e, "_raw") then
      etype, ftype, v2 = tostring(e.type), rstr(e.foreign_type), raw_json(e._raw)
    else
      etype, ftype, hint = tostring(e.type), nil, rstr(e.hint)
    end
  elseif te == "string" then
    msg = strip_where(e, rep.srcs or {}, frames)
  elseif te == "number" then
    msg = tostring(e)
  elseif te == "table" then
    local ty = rawget(e, "type")
    if type(ty) == "string" then ftype = ty
    elseif mt and type(rawget(mt, "__name")) == "string" then ftype = mt.__name end
    local m = rawget(e, "message")
    if m ~= nil then msg = tostring(m)
    elseif mt and rawget(mt, "__tostring") then
      local ok, s = pcall(tostring, e)
      msg = ok and tostring(s) or "(error object is a table value)"
    else
      msg = "(error object is a table value)"
    end
    local ok, d = pcall(simple_fields, e)
    if ok then data = d end
  else
    msg = "(error object is a " .. te .. " value)"
  end
  local parts = {'"type":' .. qtext(etype), '"message":' .. qtext(msg),
                 '"foreign_type":' .. (ftype and qtext(ftype) or "null"), '"trace":' .. frames_json(frames)}
  if data then parts[#parts + 1] = '"data":' .. data end
  if hint then parts[#parts + 1] = '"hint":' .. qtext(hint) end
  if v2 then parts[#parts + 1] = '"v2":' .. v2 end
  return "{" .. table.concat(parts, ",") .. "}"
end

local function simple_err(kind, msg)
  return '{"type":' .. quote(kind) .. ',"message":' .. qtext(msg) .. ',"foreign_type":null,"trace":[]}'
end

------------------------------------------------------------------------------------------------
-- Reverse calls (11.8, 13.6, 14.2)

local serve  -- forward

local function error_from_err(r)
  local m = getmetatable(r) == RAWOBJ and r.map or {}
  local tag = m.v2
  if getmetatable(tag) == RAWOBJ then
    local ok, e = pcall(dec, tag, 0)
    if ok and getmetatable(e) == V2ERR_MT then return e end
  end
  return v2err(rstr(m.type) or "Error", rstr(m.message) or "", rstr(m.hint))
end

host_call = function(target, name, is_route, ...)
  if STACK[#STACK] ~= "request" then
    local who = is_route and ("'" .. name .. "'") or ("V2 function '" .. name .. "'")
    error(v2err("Error", who .. " called outside an active V2 call: Lua called it while V2 wasn't waiting"
      .. " on Lua (a finalizer, or a function kept after its call returned); call V2 from the code V2 called,"
      .. " and return to V2 when done"), 0)
  end
  local a = table.pack(...)
  local parts = {}
  for i = 1, a.n do parts[i] = encode(a[i], "args[" .. (i - 1) .. "]") end
  local id = OUT_ID + 1
  local f = '{"t":"call","id":' .. id .. ',"target":' .. target .. ',"args":[' .. table.concat(parts, ",")
    .. '],"kwargs":{}}'
  if #f > LIMIT then
    error(v2err("MarshalError", string.format("value too large to send to V2 (%d MiB, limit 64 MiB)", #f // 1048576),
      HINT_BIG), 0)
  end
  OUT_ID = id
  send(f)
  if a.n < 0 then error("unreachable") end   -- keeps the arguments alive until the frame is written
  STACK[#STACK + 1] = "waiting"
  local t, m = serve(id)
  STACK[#STACK] = nil
  if t == "ret" then return to_lua(dec(m.map.value, 0)) end
  error(error_from_err(m.map.error), 0)
end

local function make_route_stub(route, name)
  local target = '{"route":' .. string.format("%d", route) .. '}'
  local f = function(...) return host_call(target, name, true, ...) end
  STUB_INFO[f] = {kind = "route", route = route, name = name}
  return f
end

local function is_callable(v)
  if type(v) == "function" then return true end
  local mt = (type(v) == "table" or type(v) == "userdata") and getmetatable(v)
  return type(mt) == "table" and rawget(mt, "__call") ~= nil
end

local function mod_get(mod, name)
  if name == "default" then return mod end
  if type(mod) == "table" then return mod[name] end
  return nil
end

local function make_same_stub(unit, name, route)
  local f = function(...)
    local u = UNITS[unit]
    if u then
      if u.kinds[name] == "value" then
        error(v2err("ImportError", "'" .. name .. "' is a constant — pass it in as a block arg"), 0)
      end
      local target
      if u.mod then target = mod_get(u.mod, name) else target = rawget(u.env, name) end
      if target == nil then error(v2err("NameError", "'" .. name .. "' is not defined in its block"), 0) end
      return target(...)
    end
    if route then return host_call('{"route":' .. string.format("%d", route) .. '}', name, true, ...) end
    error(v2err("ImportError", "'" .. name .. "' can't be called before its block loads"), 0)
  end
  STUB_INFO[f] = {kind = "same", route = route, name = name}
  return f
end

unit_target = function(unit, name)
  local u = UNITS[unit]
  if u then
    local v
    if u.mod then v = mod_get(u.mod, name) else v = rawget(u.env, name) end
    if v ~= nil then return v end
  end
  return make_same_stub(unit, name, nil)
end

------------------------------------------------------------------------------------------------
-- Request handlers

local function dir_of(p) return type(p) == "string" and p:match("^(.*)[/\\][^/\\]*$") or nil end

local function set_paths(file)
  local first = {}
  local function add(d)
    if d then
      first[#first + 1] = d .. SEP .. "?.lua"
      first[#first + 1] = d .. SEP .. "?" .. SEP .. "init.lua"
    end
  end
  local d = dir_of(file)
  add(d)
  if PROJECT_ROOT and PROJECT_ROOT ~= d then add(PROJECT_ROOT) end
  if #first == 0 then return end
  local skipped, rest = {}, {}
  for _, e in ipairs(first) do skipped[e] = true end
  for e in (package.path .. ";"):gmatch("([^;]*);") do
    if e ~= "" and not skipped[e] then rest[#rest + 1] = e end
  end
  package.path = table.concat(first, ";") .. (#rest > 0 and (";" .. table.concat(rest, ";")) or "")
end

local function check_name(n, what, lang)
  local valid = type(n) == "string" and n:find("^[%a_][%w_]*$") and load("local " .. n) ~= nil
  local reserved = n == "V2" or n == "_ENV"
  if valid and not reserved then return end
  local shown = type(n) == "string" and n or tostring(n)
  local better = (type(n) == "string" and n:find("^[%a_][%w_]*$")) and (n .. "_") or "name"
  local problem = reserved and ("is reserved in @" .. lang .. " blocks") or "is not a valid Lua name"
  if what == "arg" then
    error(v2err("ValueError", "block arg '" .. shown .. "' " .. problem .. ": pass it under another name, as in @"
      .. lang .. "(" .. better .. ": …)"), 0)
  end
  error(v2err("ValueError", "@import name '" .. shown .. "' " .. problem
    .. ": import it under another name, as in @import { … as " .. better .. " } from …"), 0)
end

local function unit_shown(unit) return (tostring(unit):gsub(":", ".", 1)) end

local function bind_import(e, env, as)
  local unit, name, route = e.unit, e.name, e.route
  if math.type(route) ~= "integer" then route = nil end
  if type(unit) ~= "string" then
    if not route then pfail("an imports entry has neither a unit nor a route") end
    return make_route_stub(route, as)
  end
  if type(name) ~= "string" then pfail("an imports entry has a unit but no name") end
  local u = UNITS[unit]
  if u then
    if u.mod then
      local v = mod_get(u.mod, name)
      if v == nil then error(v2err("ImportError", "'" .. name .. "' is not exported by '" .. unit_shown(unit) .. "'"), 0) end
      if not is_callable(v) then
        error(v2err("ImportError", "'" .. name .. "' is a constant — pass it in as a block arg",
          "a block imports only functions; in the block, import the module the language's own way: local "
          .. name .. " = require(\"" .. tostring(u.name) .. "\")." .. name), 0)
      end
      return v
    end
    if u.kinds[name] == "value" then
      error(v2err("ImportError", "'" .. name .. "' is a constant — pass it in as a block arg"), 0)
    end
    local v = rawget(u.env, name)
    if is_callable(v) then return v end
  end
  local stub = make_same_stub(unit, name, route)
  local pend = PENDING[unit]
  if not pend then pend = setmetatable({}, {__mode = "k"}); PENDING[unit] = pend end
  local binds = pend[env]
  if not binds then binds = {}; pend[env] = binds end
  binds[as] = {stub = stub, name = name}
  return stub
end

local function rebind(unit)
  local pend = PENDING[unit]
  local u = UNITS[unit]
  if not pend or not u then return end
  PENDING[unit] = nil
  for env, binds in next, pend do
    for as, b in next, binds do
      if rawget(env, as) == b.stub and u.kinds[b.name] == "func" then
        local v = rawget(u.env, b.name)
        if is_callable(v) then rawset(env, as, v) end
      end
    end
  end
end

local function compile(source, file, line, env)
  local prefix = string.rep("\n", math.max(0, line - 1))
  local chunkname = "@" .. file
  local fn = load(prefix .. "return (" .. source .. "\n)", chunkname, "t", env)
  if not fn then
    local err
    fn, err = load(prefix .. source, chunkname, "t", env)
    if not fn then
      local l, msg = tostring(err):match("^.-:(%d+): (.*)$")
      error(setmetatable({message = msg or tostring(err), file = file, line = tonumber(l) or line}, SYNTAX_MT), 0)
    end
  end
  KNOWN_SRC[debug.getinfo(fn, "S").short_src] = file
  return fn
end

local HANDLERS = {}

HANDLERS.init = function(m)
  local root = m.map.project_root
  if type(root) == "string" then PROJECT_ROOT = root end
  return ',"value":null'
end

HANDLERS.load = function(m)
  local p = m.map
  local unit, kind, lang, file, line, source = p.unit, p.kind, p.lang, p.file, p.line, p.source
  if type(lang) == "string" then TAG = lang end
  if type(source) ~= "string" or type(file) ~= "string" or math.type(line) ~= "integer" then
    pfail("a load frame without source, file or line")
  end
  set_paths(file)
  local env, nils, hasnil, argnames, imported = {}, {}, false, {}, {}
  if getmetatable(p.args) == RAWOBJ then
    for _, k in ipairs(p.args.keys) do
      check_name(k, "arg", TAG)
      argnames[k] = true
      local v = to_lua(dec(p.args.map[k], 0))
      if v == nil then nils[k] = true; hasnil = true else env[k] = v end
    end
  end
  if getmetatable(p.imports) == RAWARR then
    for i = 1, p.imports.n do
      local e = getmetatable(p.imports[i]) == RAWOBJ and p.imports[i].map or pfail("a bad imports entry")
      check_name(e.as, "import", TAG)
      local v = bind_import(e, env, e.as)
      env[e.as] = v
      imported[e.as] = v
      nils[e.as] = nil
    end
  end
  if hasnil then
    setmetatable(env, {__index = function(_, k) if nils[k] then return nil end return G[k] end})
  else
    setmetatable(env, {__index = G})
  end
  local fn = compile(source, file, line, env)
  local value = (fn())
  local vjson = "null"
  if p.want_value == true then vjson = encode(to_lua(value)) end
  local ex, missing, kinds, listed, star = {}, {}, {}, {}, false
  if getmetatable(p.exports) == RAWARR then
    for i = 1, p.exports.n do
      local name = p.exports[i]
      if name == "*" then star = true
      elseif type(name) == "string" and not listed[name] then
        listed[name] = true
        local v = rawget(env, name)
        if v == nil then
          missing[#missing + 1] = quote(name)
        elseif is_callable(v) then
          ex[#ex + 1] = '{"name":' .. quote(name) .. ',"kind":"func"}'
          kinds[name] = "func"
        else
          local ok, j = pcall(encode, v)
          if not ok then
            if getmetatable(j) == V2ERR_MT then j.message = "@export '" .. name .. "': " .. j.message end
            error(j, 0)
          end
          ex[#ex + 1] = '{"name":' .. quote(name) .. ',"kind":"value","value":' .. j .. '}'
          kinds[name] = "value"
        end
      end
    end
  end
  if star then
    local names = {}
    for k, v in next, env do
      if type(k) == "string" and not listed[k] and k:sub(1, 1) ~= "_" and type(v) == "function"
        and not argnames[k] and imported[k] ~= v then
        names[#names + 1] = k
      end
    end
    table.sort(names)
    for _, k in ipairs(names) do
      ex[#ex + 1] = '{"name":' .. quote(k) .. ',"kind":"func"}'
      kinds[k] = "func"
    end
  end
  local after
  if kind == "definition" and #missing == 0 then
    UNITS[unit] = {env = env, kinds = kinds}
    after = function() rebind(unit) end
  end
  return ',"value":' .. vjson .. ',"exports":[' .. table.concat(ex, ",") .. '],"missing":['
    .. table.concat(missing, ",") .. ']', after, {value, env}
end

local function load_module(name)
  if type(name) ~= "string" or name == "" then error(v2err("ImportError", "a Lua module needs a name"), 0) end
  if package.loaded[name] ~= nil then return package.loaded[name] end
  local found = false
  for _, searcher in ipairs(package.searchers) do
    local ok, loader = pcall(searcher, name)
    if not ok then
      local msg = tostring(loader)
      local path, l, text = msg:match("from file '(.-)':%s*.-:(%d+): (.*)$")
      if path then error(setmetatable({message = text, file = path, line = tonumber(l)}, SYNTAX_MT), 0) end
      error(msg, 0)
    end
    if type(loader) == "function" then found = true; break end
  end
  if not found then
    error(v2err("ImportError", "module '" .. name .. "' not found",
      "Lua's require found no " .. name:gsub("%.", SEP) .. ".lua (or C module) on package.path and package.cpath:"
      .. " put it next to the .v2 file or in the project folder"), 0)
  end
  return (require(name))
end

HANDLERS.module = function(m)
  local p = m.map
  local unit, name, names = p.unit, p.name, p.names
  if type(unit) == "string" then TAG = unit:match("^(.-):") or TAG end
  set_paths(p.from)
  local u = UNITS[unit]
  if not u then
    u = {mod = load_module(name), name = name, kinds = {}}
    UNITS[unit] = u
  end
  if getmetatable(names) ~= RAWARR then
    error(v2err("ImportError", "the " .. TAG .. " engine can't import a whole Lua module: that needs handles,"
      .. " which this adapter doesn't have", "import the functions you use: @import { f } from " .. TAG .. "."
      .. tostring(name)), 0)
  end
  local ex, missing = {}, {}
  for i = 1, names.n do
    local n = names[i]
    local v = type(n) == "string" and mod_get(u.mod, n) or nil
    if v == nil then
      missing[#missing + 1] = quote(tostring(n))
    elseif is_callable(v) then
      ex[#ex + 1] = '{"name":' .. quote(n) .. ',"kind":"func"}'
      u.kinds[n] = "func"
    else
      local ok, j = pcall(encode, v)
      if not ok then
        if getmetatable(j) == V2ERR_MT then j.message = "'" .. n .. "' of module " .. name .. ": " .. j.message end
        error(j, 0)
      end
      ex[#ex + 1] = '{"name":' .. quote(n) .. ',"kind":"value","value":' .. j .. '}'
      u.kinds[n] = "value"
    end
  end
  local extra = ""
  if #missing > 0 and type(u.mod) == "table" then
    local all = {}
    for k in next, u.mod do if type(k) == "string" and utf8.len(k) then all[#all + 1] = k end end
    table.sort(all)
    for i, k in ipairs(all) do all[i] = quote(k) end
    extra = ',"names":[' .. table.concat(all, ",") .. ']'
  end
  return ',"value":null,"exports":[' .. table.concat(ex, ",") .. '],"missing":[' .. table.concat(missing, ",")
    .. ']' .. extra, nil, u.mod
end

HANDLERS.call = function(m)
  local p = m.map
  local target = getmetatable(p.target) == RAWOBJ and p.target.map or pfail("a call without a target")
  if target.route ~= nil or target.fn ~= nil then pfail("V2 sent a call to a route or fn target") end
  if target.h ~= nil then pfail("V2 sent a handle call, but this adapter has no 'handles' capability") end
  if getmetatable(p.kwargs) == RAWOBJ and #p.kwargs.keys > 0 then
    pfail("V2 sent named arguments, but this adapter has no 'kwargs' capability")
  end
  local unit, name = target.unit, target.name
  CALL_NAMES[#STACK] = type(name) == "string" and name or nil
  local u = UNITS[unit]
  if not u then error(v2err("NameError", "unit " .. tostring(unit) .. " is not loaded in the Lua engine"), 0) end
  local f
  if u.mod then f = mod_get(u.mod, name) else f = rawget(u.env, name) end
  if f == nil then error(v2err("NameError", "'" .. tostring(name) .. "' is not defined in its block anymore"), 0) end
  local args, n = dec_args(p.args)
  local v = (f(table.unpack(args, 1, n)))
  return ',"value":' .. encode(v), nil, {v, args}
end

for _, f in pairs(HANDLERS) do HANDLER_FUNCS[f] = true end

local function id_json(id)
  if math.type(id) == "integer" then return string.format("%d", id) end
  if id == nil then return "null" end
  return raw_json(id)
end

local function dispatch(t, m)
  local idj = id_json(m.map.id)
  STACK[#STACK + 1] = "request"
  local ok, body, after, keep = xpcall(HANDLERS[t], on_error, m)
  local frame
  if ok then
    frame = '{"t":"ret","id":' .. idj .. body .. '}'
    if #frame > LIMIT then
      frame = '{"t":"err","id":' .. idj .. ',"error":' .. simple_err("MarshalError", string.format(
        "value too large to send to V2 (%d MiB, limit 64 MiB)", #frame // 1048576)) .. '}'
      after = nil
    end
  else
    local rep = type(body) == "table" and body.report and body or {e = body, frames = {}, srcs = {}}
    local ok2, ej = pcall(error_json, rep)
    if not ok2 then ej = simple_err("Error", "the Lua adapter could not report an error: " .. tostring(ej)) end
    frame = '{"t":"err","id":' .. idj .. ',"error":' .. ej .. '}'
  end
  send(frame)
  CALL_NAMES[#STACK] = nil
  STACK[#STACK] = nil
  if keep == STACK then error("unreachable") end   -- keeps sent values alive until written (11.8)
  if after then after() end
end

local function do_bye()
  BYE = true
  out_flush(true)
  REAL.stdout:flush()
  REAL.exit(0, true)   -- closes the state, so __gc finalizers run like exit handlers
end

serve = function(wait_id)
  while true do
    local line = REAL.stdin:read("L")
    if line == nil or line:sub(-1) ~= "\n" then REAL.exit(0, false) end   -- EOF: the host is gone
    local ok, m = pcall(json_parse, line)
    if not ok or getmetatable(m) ~= RAWOBJ or type(m.map.t) ~= "string" then
      bad_input("cannot parse a line from V2" .. (ok and "" or (": " .. tostring(m))), line)
    end
    local t, id = m.map.t, m.map.id
    if t == "ret" or t == "err" then
      if wait_id ~= nil and id == wait_id then return t, m end
      bad_input("got a reply to request " .. tostring(id) .. ", which is not the one waiting", line)
    elseif t == "release" then
      -- only `h` releases exist host -> worker, and without handles none are sent
    elseif t == "bye" then
      do_bye()
    elseif HANDLERS[t] then
      dispatch(t, m)
      if #STACK == 0 and STUBS_MADE >= 64 then
        STUBS_MADE = 0
        collectgarbage()
      end
    elseif id ~= nil and id ~= RAW_NULL then
      local what = t == "op" and "V2 sent an op, but this adapter has no 'handles' capability"
        or ("unknown frame type '" .. t .. "'")
      send('{"t":"err","id":' .. id_json(id) .. ',"error":' .. simple_err("ProtocolError", what) .. '}')
    else
      REAL.stderr:write("lua adapter: ignoring a frame of unknown type '" .. well_formed(t) .. "' without an id\n")
    end
  end
end

------------------------------------------------------------------------------------------------
-- Keeping user code off the protocol stream (9.9)

local FAKE_METHODS = {}
local FAKE_MT = {__name = "FILE*", __index = FAKE_METHODS}
FAKE_MT.__tostring = function(f) return "file (V2 " .. (f.stream or "stdin") .. ")" end
local FAKE_OUT = setmetatable({stream = "stdout"}, FAKE_MT)
local FAKE_ERR = setmetatable({stream = "stderr"}, FAKE_MT)
local FAKE_IN = setmetatable({stream = nil}, FAKE_MT)
local CUR_OUT, CUR_IN = FAKE_OUT, FAKE_IN

function FAKE_METHODS.write(self, ...)
  if not self.stream then return nil, "Bad file descriptor", 9 end
  for i = 1, select("#", ...) do
    local v = select(i, ...)
    local tv = type(v)
    local s
    if tv == "string" then s = v
    elseif tv == "number" then s = math.type(v) == "integer" and string.format("%d", v) or string.format("%.14g", v)
    else error(string.format("bad argument #%d to 'write' (string expected, got %s)", i, tv), 2) end
    out_write(self.stream, s)
  end
  return self
end
function FAKE_METHODS.read(self)
  if self.stream then return nil, "Bad file descriptor", 9 end
  error(STDIN_MSG, 2)
end
function FAKE_METHODS.lines(self)
  if self.stream then error("file is not readable", 2) end
  error(STDIN_MSG, 2)
end
function FAKE_METHODS.flush(self)
  if BYE or STACK[#STACK] == "request" then out_flush(false) end
  return self
end
function FAKE_METHODS.close() return nil, "cannot close standard file" end
function FAKE_METHODS.setvbuf() return true end
function FAKE_METHODS.seek() return nil, "Illegal seek", 29 end

local function is_fake(f) return getmetatable(f) == FAKE_MT end

G.print = function(...)
  local n = select("#", ...)
  local parts = {}
  for i = 1, n do parts[i] = tostring((select(i, ...))) end
  out_write("stdout", table.concat(parts, "\t", 1, n) .. "\n")
end
io.stdout, io.stderr, io.stdin = FAKE_OUT, FAKE_ERR, FAKE_IN
io.write = function(...) return CUR_OUT:write(...) end
io.read = function(...) return CUR_IN:read(...) end
io.lines = function(name, ...)
  if name == nil then return CUR_IN:lines(...) end
  return REAL.lines(name, ...)
end
io.output = function(f)
  if type(f) == "string" then
    local h, err = REAL.open(f, "w")
    if not h then error(err, 2) end
    CUR_OUT = h
  elseif f ~= nil then
    CUR_OUT = f
  end
  return CUR_OUT
end
io.input = function(f)
  if type(f) == "string" then
    local h, err = REAL.open(f, "r")
    if not h then error(err, 2) end
    CUR_IN = h
  elseif f ~= nil then
    CUR_IN = f
  end
  return CUR_IN
end
io.close = function(f)
  f = f or CUR_OUT
  return f:close()
end
io.type = function(f)
  if is_fake(f) then return "file" end
  return REAL.iotype(f)
end
io.popen = function(cmd, mode)
  mode = mode or "r"
  if type(cmd) == "string" then
    if WINDOWS then
      cmd = "(" .. cmd .. ")" .. (mode:sub(1, 1) == "w" and " 1>&2" or " <NUL")
    else
      cmd = (mode:sub(1, 1) == "w" and "exec 1>&2\n" or "exec 0</dev/null\n") .. cmd
    end
  end
  return REAL.popen(cmd, mode)
end
os.execute = function(cmd)
  if type(cmd) ~= "string" then return REAL.execute(cmd) end
  if WINDOWS then return REAL.execute("(" .. cmd .. ") <NUL 1>&2") end
  return REAL.execute("exec 0</dev/null 1>&2\n" .. cmd)
end
os.exit = function(...)
  local code = ...
  local shown = select("#", ...) == 0 and "" or tostring(code)
  local n = (code == nil or code == true) and 0 or (code == false and 1) or math.tointeger(code) or 1
  error({message = "os.exit(" .. shown .. ") can't end V2's Lua engine; raise an error instead, or end the program"
    .. " with V2's exit(" .. n .. ")", exit_code = n}, 2)
end
debug.debug = function() error(STDIN_MSG, 2) end

G.V2 = {
  null = NULL,
  list = function(t) return setmetatable(t or {}, LIST_MT) end,
  dict = function(t) return setmetatable(t or {}, {__v2 = "dict", __name = "v2.dict"}) end,
  bytes = function(s)
    if type(s) ~= "string" then error("V2.bytes takes a string", 2) end
    return setmetatable({data = s}, BYTES_MT)
  end,
  isbytes = function(x) return type(x) == "table" and getmetatable(x) == BYTES_MT end,
  iserror = function(x) return type(x) == "table" and getmetatable(x) == V2ERR_MT end,
}

------------------------------------------------------------------------------------------------
-- Start: hello (9.3), then serve

local function own_pid()
  local f = REAL.open("/proc/self/stat", "r")
  if f then
    local n = f:read("n")
    f:close()
    if math.type(n) == "integer" then return n end
  end
  if not WINDOWS then
    local p = REAL.popen("exec 0</dev/null; echo $PPID", "r")
    if p then
      local n = p:read("n")
      p:close()
      if math.type(n) == "integer" then return n end
    end
  end
  return 0
end

REAL.stdout:setvbuf("full")
raw_write('{"t":"hello","vep":1,"version":' .. quote(_VERSION:match("%d+%.%d+") or _VERSION) .. ',"pid":'
  .. own_pid() .. ',"caps":["modules","reverse"]}')
serve(nil)
