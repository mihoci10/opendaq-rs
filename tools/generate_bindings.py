#!/usr/bin/env python3
"""generate_bindings.py - Generate the Rust binding sources from the openDAQ C headers.

Usage:
    python generate_bindings.py --opendaq-repo tmp/openDAQ [--output-dir src]

Emits:
  src/sys/generated.rs   - scalar/opaque/callback types, enums, error-code
                           constants, the Api function-pointer table with its
                           symbol resolver, and the callback trampoline pools.
  src/generated/*.rs     - the safe high-level layer: one #[repr(transparent)]
                           struct per interface, Deref-chained to its parent,
                           with Result-returning methods for every C function.

The parsing itself is delegated to parse_bindings.py; this file only decides
how the parsed model maps onto Rust.  Functions whose signatures the mechanical mapping cannot model (raw
void* sample buffers, in-out parameters, C callback parameters) are skipped
here and covered by hand-written wrappers in the crate (see the SKIP report
printed at the end).
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

# Scalar typedefs from ccommon.h, mapped by hand (stable, and the parser does
# not preserve constness for aliases).
SCALAR_ALIASES = {
    "daqErrCode": "u32",
    "daqBool": "u8",
    "daqInt": "i64",
    "daqUInt": "u64",
    "daqFloat": "f64",
    "daqCharPtr": "*mut c_char",
    "daqConstCharPtr": "*const c_char",
    "daqVoidPtr": "*mut c_void",
    "daqSizeT": "usize",
    "daqEnumType": "u32",
}

BUILTIN_TO_RUST = {
    "char": "c_char",
    "double": "f64",
    "float": "f32",
    "int": "c_int",
    "int8_t": "i8",
    "int16_t": "i16",
    "int32_t": "i32",
    "int64_t": "i64",
    "size_t": "usize",
    "uint8_t": "u8",
    "uint16_t": "u16",
    "uint32_t": "u32",
    "uint64_t": "u64",
}

# The C callback typedefs (ccommon.h).  parse_bindings.py records their names
# but not their signatures, so they are spelled out here; an unknown callback
# typedef aborts generation.
CALLBACK_TYPES = {
    "daqFuncCall": "unsafe extern \"C\" fn(params: *mut daqBaseObject, result: *mut *mut daqBaseObject) -> daqErrCode",
    "daqProcCall": "unsafe extern \"C\" fn(params: *mut daqBaseObject) -> daqErrCode",
    "daqEventCall": "unsafe extern \"C\" fn(sender: *mut daqBaseObject, args: *mut daqBaseObject)",
}

# Exported with extern "C" linkage from the core libraries but not declared in
# the bindings/c headers; injected into the Api table by hand.
EXTRA_API_FUNCTIONS = [
    ("daqGetErrorInfoMessage", "unsafe extern \"C\" fn(message: *mut *mut daqString) -> daqErrCode"),
    ("daqClearErrorInfo", "unsafe extern \"C\" fn()"),
    ("daqFreeMemory", "unsafe extern \"C\" fn(ptr: *mut c_void)"),
    ("daqAllocateMemory", "unsafe extern \"C\" fn(len: usize) -> *mut c_void"),
]

# Interface renames applied on top of the mechanical daqXxx -> Xxx stripping.
# The boxed scalar wrappers get an -Object suffix (a bare `String` / `List`
# would collide with std types in user imports), and daqIterator would shadow
# the std::iter::Iterator trait in any glob import.
INTERFACE_RENAMES = {
    "daqString": "StringObject",
    "daqInteger": "IntegerObject",
    "daqBoolean": "BooleanObject",
    "daqRatio": "RatioObject",
    "daqComplexNumber": "ComplexNumberObject",
    "daqNumber": "NumberObject",
    "daqList": "ListObject",
    "daqDict": "DictObject",
    "daqFunction": "FunctionObject",
    "daqIterator": "ObjectIterator",
}

# Interfaces excluded from high-level generation.  The typed sample readers
# are hand-written as generic types (StreamReader<V, D>, ...) in the crate --
# their read() surface needs runtime-sized untyped buffers and in-out counts
# the mechanical mapping cannot model -- and they absorb their builders and
# the shared reader base interfaces.
EXCLUDED_INTERFACES = {
    "daqReader",
    "daqSampleReader",
    "daqStreamReader",
    "daqTailReader",
    "daqBlockReader",
    "daqMultiReader",
    "daqPacketReader",
    "daqStreamReaderBuilder",
    "daqTailReaderBuilder",
    "daqBlockReaderBuilder",
    "daqMultiReaderBuilder",
}

# Receivers whose functions are entirely hand-written (crate root modules).
MANUAL_RECEIVERS = {
    "daqBaseObject",  # object.rs: refcounting, casting, equality, toString
}

# Individual C functions excluded from high-level generation: hand-written,
# redundant, or deliberately unmapped.  Everything the *mechanical* rules
# cannot model (void* buffers, in-out counts, callback parameters) is skipped
# automatically and needs no entry here.
MANUAL_FUNCTIONS = {
    # data.rs: runtime-typed sample buffers
    "daqDataPacket_getData",
    "daqDataPacket_getRawData",
    # instance.rs: Instance::new() defaults the builder's module path to the
    # bundled native modules
    "daqInstance_createInstance",
    "daqInstanceBuilder_createInstanceBuilder",  # generated normally; listed for clarity
    # exact duplicates of the canonical boxed-scalar constructors
    "daqFloatObject_createFloat",
    "daqBoolean_createBoolean",
    # returns two values (context + intf-id GUID); internal deserialization
    # plumbing unused by the high level
    "daqComponentDeserializeContext_createComponentDeserializeContext",
}
MANUAL_FUNCTIONS.discard("daqInstanceBuilder_createInstanceBuilder")

# C functions whose pointer-to-scalar parameter is in-out rather than out;
# unmodellable mechanically, all covered by the hand-written readers.
IN_OUT_FUNCTIONS = {
    "daqBlockReader_read",
    "daqBlockReader_readWithDomain",
    "daqConnectionInternal_dequeueUpTo",
    "daqFunction_call",
    "daqMultiReader_read",
    "daqMultiReader_readWithDomain",
    "daqMultiReader_skipSamples",
    "daqStreamReader_read",
    "daqStreamReader_readWithDomain",
    "daqStreamReader_skipSamples",
    "daqTailReader_read",
    "daqTailReader_readWithDomain",
}

# Constructor-proxy names (Type::name) for factory functions whose C name does
# not embed the type they build, so the mechanical suffix derivation fails or
# yields something misleading.
CONSTRUCTOR_NAME_OVERRIDES = {
    "daqComponentTypeBuilder_createDeviceTypeBuilder": "device",
    "daqComponentTypeBuilder_createFunctionBlockTypeBuilder": "function_block",
    "daqComponentTypeBuilder_createServerTypeBuilder": "server",
    "daqComponentTypeBuilder_createStreamingTypeBuilder": "streaming",
    "daqSignalConfig_createSignalWithDescriptor": "with_descriptor",
}

# Parameters that accept a null pointer even though the C headers carry no
# [defaultValue(nullptr)] annotation for them (the other bindings pass nil
# there); marked optional so the Rust signature takes an Option.
OPTIONAL_PARAM_OVERRIDES = {
    "daqSignalConfig_createSignal": {"parent", "className"},
    "daqSignalConfig_createSignalWithDescriptor": {"parent", "className"},
    "daqComponent_createComponent": {"parent", "className"},
    "daqInputPortConfig_createInputPort": {"parent"},
    "daqContext_createContext": {"Scheduler", "moduleManager", "authenticationProvider"},
}

RUST_KEYWORDS = {
    "as", "async", "await", "box", "break", "const", "continue", "crate", "dyn",
    "else", "enum", "extern", "false", "fn", "for", "if", "impl", "in", "let",
    "loop", "match", "mod", "move", "mut", "priv", "pub", "ref", "return",
    "self", "static", "struct", "super", "trait", "true", "try", "type",
    "unsafe", "use", "where", "while", "yield",
}

# Callback trampoline pool size per callback kind (see src/callbacks.rs).
TRAMPOLINE_POOL_SIZE = 256

# The `mod` declarations naming these files carry #[rustfmt::skip], keeping
# `cargo fmt` off the generated output (an inner #![rustfmt::skip] would be
# nicer but custom inner attributes are unstable).
HEADER = "// Generated by tools/generate_bindings.py -- do not edit by hand.\n"


# ---------------------------------------------------------------------------
# Small name helpers
# ---------------------------------------------------------------------------

def camel_to_snake(name: str) -> str:
    tokens = re.findall(r"[A-Z]+(?=[A-Z][a-z]|[0-9]|$)|[A-Z]?[a-z]+|[0-9]+", name)
    return "_".join(t.lower() for t in tokens if t)


def sanitize_ident(name: str) -> str:
    return name + "_" if name in RUST_KEYWORDS else name


def pascal_case(snakeish: str) -> str:
    return "".join(part.capitalize() for part in re.split(r"[_\-]", snakeish) if part)


def rust_interface_name(c_name: str) -> str:
    if c_name in INTERFACE_RENAMES:
        return INTERFACE_RENAMES[c_name]
    assert c_name.startswith("daq"), c_name
    return c_name[3:]


def doc_lines(text: str, indent: str = "") -> list[str]:
    """Turn a raw doxygen block into /// doc comment lines, defusing what
    rustdoc would read as markdown links or HTML tags."""
    text = re.sub(r"^@brief\s+", "", text.strip())
    if not text:
        return []
    lines = []
    for line in text.split("\n"):
        line = (
            line.replace("[", "\\[")
            .replace("]", "\\]")
            .replace("<", "\\<")
            .replace(">", "\\>")
            .rstrip()
        )
        lines.append(f"{indent}/// {line}".rstrip())
    return lines


# ---------------------------------------------------------------------------
# Parsing / model
# ---------------------------------------------------------------------------

def parse_records(repo: Path) -> list[dict]:
    output = subprocess.run(
        [sys.executable, str(Path(__file__).with_name("parse_bindings.py")),
         "--opendaq-repo", str(repo)],
        check=True, capture_output=True, text=True,
    ).stdout
    return [json.loads(line) for line in output.splitlines() if line.strip()]


class Model:
    def __init__(self, records: list[dict]):
        self.typedefs: dict[str, dict] = {}
        for r in records:
            if r["kind"] == "typedef" and r["name"] not in self.typedefs:
                self.typedefs[r["name"]] = r
        self.interface_parent: dict[str, str] = {}
        for r in records:
            if r["kind"] == "interface":
                self.interface_parent[r["name"]] = r["parent"]
        self.interface_docs: dict[str, str] = {
            r["name"]: r.get("docstring", "") for r in records if r["kind"] == "interface"
        }
        self.interface_sources: dict[str, str] = {
            r["name"]: r["source_file"] for r in records if r["kind"] == "interface"
        }
        seen_functions: dict[str, dict] = {}
        for r in records:
            if r["kind"] == "function" and r["name"] not in seen_functions:
                seen_functions[r["name"]] = r
        self.functions: list[dict] = sorted(seen_functions.values(), key=lambda r: r["name"])
        # Deduplicate error codes by name; conflicting redefinitions abort.
        codes: dict[str, int] = {}
        for r in records:
            if r["kind"] != "error_code":
                continue
            if r["name"] in codes and codes[r["name"]] != r["code"]:
                raise ValueError(f"conflicting error code {r['name']}")
            codes[r["name"]] = r["code"]
        self.error_codes = sorted(codes.items(), key=lambda kv: (kv[1], kv[0]))

        self.opaque: set[str] = {
            n for n, t in self.typedefs.items() if t["category"] == "opaque"
        }
        # Marker interfaces (daqChannel, daqIoFolderConfig, ...) declare no
        # typedef of their own -- only the DECLARE_OPENDAQ_INTERFACE comment and
        # a getInterfaceId; register them as opaque interfaces anyway.
        for name in self.interface_parent:
            if name != "daqBaseObject" and name not in self.typedefs:
                self.opaque.add(name)
        self.enums: dict[str, dict] = {
            n: t for n, t in self.typedefs.items() if t["category"] == "enum"
        }
        self.structs: dict[str, dict] = {
            n: t for n, t in self.typedefs.items() if t["category"] == "struct"
        }
        self.callbacks: set[str] = {
            n for n, t in self.typedefs.items() if t["category"] == "callback"
        }
        unknown_callbacks = self.callbacks - set(CALLBACK_TYPES)
        if unknown_callbacks:
            raise ValueError(f"unknown callback typedefs: {unknown_callbacks}")
        unknown_structs = set(self.structs) - {"daqIntfID"}
        if unknown_structs:
            raise ValueError(f"unknown by-value structs: {unknown_structs}")

        # Aliases of opaque interfaces (e.g. daqCoreTypeObject) -> target name.
        self.opaque_aliases: dict[str, str] = {}
        for n, t in self.typedefs.items():
            if t["category"] == "opaque" and t.get("base_type") in self.opaque:
                self.opaque_aliases[n] = t["base_type"]

        # Functions grouped by receiver (the prefix before the first '_').
        self.by_receiver: dict[str, list[dict]] = defaultdict(list)
        for f in self.functions:
            receiver = f["name"].partition("_")[0]
            self.by_receiver[receiver].append(f)

        self.has_interface_id: set[str] = {
            f["name"].partition("_")[0]
            for f in self.functions
            if f["name"].endswith("_getInterfaceId")
        }

    def canon(self, name: str) -> str:
        """Resolve alias typedefs to a canonical type name, stopping at the
        hand-mapped scalar aliases (daqBool must stay daqBool, not decay to
        uint8_t) and folding opaque-interface aliases onto their target."""
        seen = set()
        while (name not in SCALAR_ALIASES and name != "daqBaseObject"
               and name in self.typedefs
               and self.typedefs[name]["category"] == "alias"
               and self.typedefs[name].get("base_type")
               and name not in seen):
            seen.add(name)
            name = self.typedefs[name]["base_type"]
        return self.opaque_aliases.get(name, name)

    def module_of(self, source_file: str) -> str:
        parts = Path(source_file).parts
        if "ccoretypes" in parts:
            return "coretypes"
        if "ccoreobjects" in parts:
            return "coreobjects"
        if "copendaq" in parts:
            idx = parts.index("copendaq")
            if idx + 2 < len(parts):
                return parts[idx + 1]
        return "common"


# ---------------------------------------------------------------------------
# Enum model
# ---------------------------------------------------------------------------

def strip_common_prefix(names: list[str]) -> list[str]:
    """Drop the longest common leading token run so daqSampleTypeFloat64
    becomes Float64 (backing off if a result would start with a digit)."""
    if len(names) < 2:
        # daqXxxYyy single-entry enums: strip nothing reliable; keep last token.
        return names
    token_lists = [re.findall(r"[A-Z]+(?=[A-Z][a-z]|[0-9]|$)|[A-Z]?[a-z]+|[0-9]+", n) for n in names]
    shortest = min(len(t) for t in token_lists)
    common = 0
    while common < shortest and len({t[common] for t in token_lists}) == 1:
        common += 1
    common = min(common, shortest - 1)
    while common > 0:
        candidates = ["".join(t[common:]) for t in token_lists]
        if all(c and not c[0].isdigit() for c in candidates):
            break
        common -= 1
    return ["".join(t[common:]) for t in token_lists]


class EnumModel:
    def __init__(self, c_name: str, record: dict):
        self.c_name = c_name
        self.rust_name = rust_interface_name(c_name) if c_name.startswith("daq") else c_name
        entries = record.get("enum_entries") or []
        usable = [(e["name"], e["value"]) for e in entries if e["value"] is not None]
        self.dropped = [e["name"] for e in entries if e["value"] is None]
        variant_names = strip_common_prefix([n for n, _ in usable])
        self.variants: list[tuple[str, int]] = []      # unique values
        self.aliases: list[tuple[str, str]] = []       # duplicate values -> first variant
        seen_values: dict[int, str] = {}
        seen_names: set[str] = set()
        for raw, (orig, value) in zip(variant_names, usable):
            name = pascal_case(camel_to_snake(raw)) if not raw[0].isdigit() else f"V{raw}"
            if name in seen_names:
                name = pascal_case(camel_to_snake(orig))
            seen_names.add(name)
            if value in seen_values:
                self.aliases.append((name, seen_values[value]))
            else:
                seen_values[value] = name
                self.variants.append((name, value))


# ---------------------------------------------------------------------------
# sys layer emission
# ---------------------------------------------------------------------------

def sys_type(model: Model, base: str, depth: int, const: bool) -> str:
    """The Rust spelling of a C type in the Api function-pointer table."""
    base = model.canon(base)

    def wrap(inner: str, levels: int) -> str:
        for _ in range(levels):
            inner = f"*{'const' if const else 'mut'} {inner}"
        return inner

    if base in SCALAR_ALIASES:
        return wrap(base, depth)
    if base == "daqBaseObject":
        return wrap("daqBaseObject", depth) if depth else "daqBaseObject"
    if base in model.opaque or base in model.callbacks:
        if base in model.callbacks and depth == 0:
            return base
        return wrap(base, depth)
    if base in model.enums:
        return wrap("u32", depth)
    if base == "daqIntfID":
        return wrap("daqIntfID", depth)
    if base in BUILTIN_TO_RUST:
        return wrap(BUILTIN_TO_RUST[base], depth)
    if base == "void":
        if depth == 0:
            return "()"
        return wrap("c_void", depth)
    raise ValueError(f"unmapped C type {base}")


def api_fn_signature(model: Model, function: dict) -> tuple[str, str]:
    """(parameter list, return type) of a C function as Rust spellings."""
    params = []
    for arg in function.get("arguments", []):
        t = arg["type"]
        rust = sys_type(model, t["name"], t.get("pointer_depth", 0), t.get("is_const", False))
        params.append(f"{sanitize_ident(arg['name'])}: {rust}")
    ret = function["return_type"]
    ret_rust = sys_type(model, ret["name"], ret.get("pointer_depth", 0), False)
    return ", ".join(params), ret_rust


def api_fn_type(model: Model, function: dict) -> str:
    params, ret = api_fn_signature(model, function)
    suffix = "" if ret == "()" else f" -> {ret}"
    return f"unsafe extern \"C\" fn({params}){suffix}"


def stub_body(ret: str) -> str:
    """Body of a missing-symbol stub returning a harmless value: NOTIMPLEMENTED
    for status codes, zero / null / nothing otherwise."""
    if ret == "daqErrCode":
        return "OPENDAQ_ERR_NOTIMPLEMENTED"
    if ret == "()":
        return ""
    if ret.startswith("*"):
        return "std::ptr::null_mut()"
    if ret in ("f32", "f64"):
        return "0.0"
    return "0"


def emit_sys(model: Model) -> str:
    lines: list[str] = [
        HEADER,
        "#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals, dead_code)]",
        "#![allow(clippy::all)]",
        "",
        "use std::ffi::{c_char, c_int, c_void};",
        "",
        "use super::daqIntfID;",
        "",
        "// --- Scalar typedefs (ccommon.h) ---",
    ]
    for name, rust in SCALAR_ALIASES.items():
        lines.append(f"pub type {name} = {rust};")
    lines.append("pub type daqBaseObject = c_void;")
    lines.append("")

    lines.append("// --- Callback typedefs ---")
    for name, sig in CALLBACK_TYPES.items():
        lines.append(f"pub type {name} = {sig};")
    lines.append("")

    lines.append("// --- Opaque interface types ---")
    for name in sorted(model.opaque):
        if name in model.opaque_aliases:
            continue
        lines.append("#[repr(C)]")
        lines.append(f"pub struct {name} {{ _opaque: [u8; 0] }}")
    for alias, target in sorted(model.opaque_aliases.items()):
        lines.append(f"pub type {alias} = {target};")
    lines.append("")

    lines.append("// --- Enums ---")
    enum_models = [EnumModel(n, t) for n, t in sorted(model.enums.items())]
    for em in enum_models:
        lines.append("#[repr(u32)]")
        lines.append("#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]")
        lines.append("#[non_exhaustive]")
        lines.append(f"pub enum {em.rust_name} {{")
        for name, value in em.variants:
            lines.append(f"    {name} = {value},")
        lines.append("}")
        lines.append(f"impl {em.rust_name} {{")
        for alias, target in em.aliases:
            lines.append(f"    pub const {alias}: {em.rust_name} = {em.rust_name}::{target};")
        lines.append("    pub fn from_raw(value: u32) -> Option<Self> {")
        lines.append("        match value {")
        for name, value in em.variants:
            lines.append(f"            {value} => Some(Self::{name}),")
        lines.append("            _ => None,")
        lines.append("        }")
        lines.append("    }")
        lines.append("}")
        lines.append(f"impl From<{em.rust_name}> for u32 {{")
        lines.append(f"    fn from(value: {em.rust_name}) -> u32 {{ value as u32 }}")
        lines.append("}")
        for dropped in em.dropped:
            lines.append(f"// note: enum entry {dropped} has a non-integer value and was dropped")
        lines.append(f"pub type {em.c_name} = {em.rust_name};")
        lines.append("")

    lines.append("// --- Error codes ---")
    for name, code in model.error_codes:
        lines.append(f"pub const {name}: u32 = 0x{code:08X};")
    lines.append("")
    lines.append("/// The upstream symbolic name of a status code, when known.")
    lines.append("pub fn error_code_name(code: u32) -> Option<&'static str> {")
    lines.append("    match code {")
    seen_codes: set[int] = set()
    for name, code in model.error_codes:
        if code in seen_codes:
            continue
        seen_codes.add(code)
        lines.append(f"        0x{code:08X} => Some(\"{name}\"),")
    lines.append("        _ => None,")
    lines.append("    }")
    lines.append("}")
    lines.append("")

    lines.append("// --- The resolved C API ---")
    lines.append("")
    lines.append("/// Every function of the openDAQ flat C API, resolved from the loaded")
    lines.append("/// native libraries.  Obtain it through [`super::api`].")
    lines.append("///")
    lines.append("/// A symbol the loaded libraries do not export (the headers also describe")
    lines.append("/// private interfaces some builds omit) resolves to a stub that reports")
    lines.append("/// `OPENDAQ_ERR_NOTIMPLEMENTED`, so loading never fails on version skew and")
    lines.append("/// only actually calling the missing function surfaces an error.")
    lines.append("pub struct Api {")
    entries: list[tuple[str, str]] = []
    for f in model.functions:
        params, ret = api_fn_signature(model, f)
        suffix = "" if ret == "()" else f" -> {ret}"
        entries.append((f["name"], f"unsafe extern \"C\" fn({params}){suffix}", params, ret))
    for name, fn_type in EXTRA_API_FUNCTIONS:
        inner = fn_type[len("unsafe extern \"C\" fn("):]
        params, _, ret_part = inner.partition(")")
        ret = ret_part.strip().lstrip("->").strip() or "()"
        entries.append((name, fn_type, params, ret))
    for name, fn_type, _, _ in entries:
        lines.append(f"    pub {name}: {fn_type},")
    lines.append("}")
    lines.append("")
    lines.append("/// Stubs standing in for symbols the loaded libraries do not export.")
    lines.append("mod missing {")
    lines.append("    use super::*;")
    for name, _, params, ret in entries:
        anon_params = ", ".join(
            f"_: {p.partition(': ')[2]}" for p in params.split(", ") if p
        )
        suffix = "" if ret == "()" else f" -> {ret}"
        body = stub_body(ret)
        lines.append(f"    pub(super) unsafe extern \"C\" fn {name}({anon_params}){suffix} {{ {body} }}")
    lines.append("}")
    lines.append("")
    lines.append("impl Api {")
    lines.append("    pub(crate) fn resolve(")
    lines.append("        libraries: &[libloading::Library],")
    lines.append("    ) -> Result<Api, crate::loader::LoadError> {")
    lines.append("        let s = |name: &[u8]| crate::loader::resolve_symbol(libraries, name);")
    lines.append("        unsafe {")
    lines.append("            Ok(Api {")
    for name, _, _, _ in entries:
        lines.append(
            f"                {name}: match s(b\"{name}\\0\") {{ Ok(p) => std::mem::transmute(p), Err(_) => missing::{name} }},"
        )
    lines.append("            })")
    lines.append("        }")
    lines.append("    }")
    lines.append("}")
    lines.append("")

    lines.append("// --- Callback trampoline pools ---")
    lines.append("//")
    lines.append("// The C callback types carry no user-data pointer, so every distinct Rust")
    lines.append("// closure handed to openDAQ needs its own native entry point.  A fixed pool")
    lines.append("// of pre-compiled trampolines routes each call, via its baked-in slot index,")
    lines.append("// to the closure registry in crate::callbacks.")
    lines.append(f"pub const TRAMPOLINE_POOL_SIZE: usize = {TRAMPOLINE_POOL_SIZE};")
    for kind, c_type, params, forward in (
        ("event", "daqEventCall", "sender: *mut daqBaseObject, args: *mut daqBaseObject",
         "crate::callbacks::dispatch_event(INDEX, sender, args)"),
        ("procedure", "daqProcCall", "params: *mut daqBaseObject",
         "return crate::callbacks::dispatch_procedure(INDEX, params)"),
        ("function", "daqFuncCall", "params: *mut daqBaseObject, result: *mut *mut daqBaseObject",
         "return crate::callbacks::dispatch_function(INDEX, params, result)"),
    ):
        ret = " -> daqErrCode" if kind != "event" else ""
        for index in range(TRAMPOLINE_POOL_SIZE):
            body = forward.replace("INDEX", str(index))
            lines.append(
                f"unsafe extern \"C\" fn {kind}_trampoline_{index}({params}){ret} {{ {body}; }}"
            )
        lines.append(f"pub static {kind.upper()}_TRAMPOLINES: [{c_type}; TRAMPOLINE_POOL_SIZE] = [")
        for index in range(TRAMPOLINE_POOL_SIZE):
            lines.append(f"    {kind}_trampoline_{index},")
        lines.append("];")
    lines.append("")
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# High-level model: argument and result conversion strategies
# ---------------------------------------------------------------------------

class Skip(Exception):
    pass


def element_rust(name: str) -> tuple[str, str]:
    """Map an elementType/templateType annotation name (e.g. "Signal",
    "String") to (rust element type, conversion kind)."""
    if name in ("String",):
        return "String", "string"
    if name in ("Integer",):
        return "i64", "int"
    if name in ("Float",):
        return "f64", "float"
    if name in ("Bool", "Boolean"):
        return "bool", "bool"
    if name in ("BaseObject",):
        return "Value", "value"
    c_name = "daq" + name
    return rust_interface_name(c_name), "interface"


class Param:
    """One high-level input parameter with its marshaling strategy."""

    def __init__(self, model: Model, arg: dict, function: dict):
        self.c_name = arg["name"]
        self.rust_name = sanitize_ident(camel_to_snake(arg["name"]))
        t = arg["type"]
        base = t["name"]
        resolved = model.canon(base)
        depth = t.get("pointer_depth", 0)
        self.default = arg.get("default_value")
        self.optional = self.default in ("nullptr", "NULL") or (
            arg["name"] in OPTIONAL_PARAM_OVERRIDES.get(function["name"], ())
        )
        self.kind = None
        self.rust_type = None

        if resolved in model.callbacks:
            raise Skip("callback parameter")
        if depth == 0:
            if resolved in ("daqCharPtr", "daqConstCharPtr"):
                # A raw C string, NOT an IString object pointer.
                self.kind, self.rust_type = "c_str", "&str"
            elif resolved in model.enums:
                self.kind, self.rust_type = "enum", EnumModel(resolved, model.enums[resolved]).rust_name
            elif resolved == "daqIntfID":
                self.kind, self.rust_type = "intf_id", "crate::IntfID"
            elif resolved == "daqBool":
                self.kind, self.rust_type = "bool", "bool"
            elif resolved in SCALAR_ALIASES and SCALAR_ALIASES[resolved] in (
                    "i64", "u64", "f64", "usize", "u32"):
                self.kind, self.rust_type = "scalar", SCALAR_ALIASES[resolved]
            elif resolved in BUILTIN_TO_RUST:
                self.kind, self.rust_type = "scalar", BUILTIN_TO_RUST[resolved]
            else:
                raise Skip(f"by-value parameter of type {base}")
            return
        if depth != 1:
            raise Skip(f"unexpected pointer depth on input {base}")
        if base in ("daqCharPtr", "daqConstCharPtr"):
            raise Skip("char** input")
        if resolved == "char":
            # A raw `const char*` spelled without the alias.
            self.kind, self.rust_type = "c_str", "&str"
        elif resolved == "daqString":
            if self.optional and self.default is None:
                self.kind, self.rust_type = "opt_str", "Option<&str>"
            else:
                self.kind, self.rust_type = "str", "&str"
        elif resolved == "daqBaseObject":
            self.kind, self.rust_type = "value", "impl Into<Value>"
        elif resolved == "daqNumber":
            self.kind, self.rust_type = "number", "impl Into<Value>"
        elif resolved == "daqRatio":
            self.kind, self.rust_type = "ratio", "Ratio"
        elif resolved == "daqComplexNumber":
            self.kind, self.rust_type = "complex", "Complex"
        elif resolved == "daqInteger":
            self.kind, self.rust_type = "boxed_int", "i64"
        elif resolved == "daqFloatObject":
            self.kind, self.rust_type = "boxed_float", "f64"
        elif resolved == "daqBoolean":
            self.kind, self.rust_type = "boxed_bool", "bool"
        elif resolved == "daqList":
            element = t.get("value_type")
            if element is None:
                self.kind, self.rust_type = "value", "impl Into<Value>"
            else:
                elem_rust, elem_kind = element_rust(element)
                if elem_kind == "interface":
                    if "daq" + element in EXCLUDED_INTERFACES:
                        raise Skip(f"list of excluded interface {element}")
                    self.kind, self.rust_type = "interface_slice", f"&[{elem_rust}]"
                elif elem_kind == "string":
                    self.kind, self.rust_type = "str_slice", "&[&str]"
                else:
                    self.kind, self.rust_type = "value", "impl Into<Value>"
        elif resolved == "daqDict":
            self.kind, self.rust_type = "value", "impl Into<Value>"
        elif resolved in model.opaque:
            if resolved in EXCLUDED_INTERFACES:
                raise Skip(f"parameter of excluded interface {resolved}")
            rust = rust_interface_name(resolved)
            if self.optional:
                self.kind, self.rust_type = "opt_interface", f"Option<&{rust}>"
            else:
                self.kind, self.rust_type = "interface", f"&{rust}"
        elif resolved == "void":
            raise Skip("raw void* parameter")
        elif resolved == "daqIntfID":
            raise Skip("daqIntfID* input")
        else:
            raise Skip(f"unmapped input type {base}*")

    def signature(self) -> str:
        if self.kind == "value" or self.kind == "number":
            return f"{self.rust_name}: {self.rust_type}"
        return f"{self.rust_name}: {self.rust_type}"

    def prologue(self) -> list[str]:
        """Statements converting the Rust argument into FFI-ready locals."""
        n = self.rust_name
        if self.kind == "str":
            return [f"let __{n} = crate::marshal::make_string({n})?;"]
        if self.kind == "c_str":
            return [f"let __{n} = crate::marshal::make_c_string({n})?;"]
        if self.kind == "opt_str":
            return [
                f"let __{n} = match {n} {{ Some(s) => Some(crate::marshal::make_string(s)?), None => None }};"
            ]
        if self.kind == "value":
            return [f"let __{n} = crate::value::to_daq(&{n}.into())?;"]
        if self.kind == "number":
            return [f"let __{n} = crate::value::to_daq_number(&{n}.into())?;"]
        if self.kind == "ratio":
            return [f"let __{n} = crate::value::ratio_to_ref({n})?;"]
        if self.kind == "complex":
            return [f"let __{n} = crate::value::complex_to_ref({n})?;"]
        if self.kind == "boxed_int":
            return [f"let __{n} = crate::value::int_to_ref({n})?;"]
        if self.kind == "boxed_float":
            return [f"let __{n} = crate::value::float_to_ref({n})?;"]
        if self.kind == "boxed_bool":
            return [f"let __{n} = crate::value::bool_to_ref({n})?;"]
        if self.kind == "interface_slice":
            return [f"let __{n} = crate::marshal::list_from_interfaces({n})?;"]
        if self.kind == "str_slice":
            return [f"let __{n} = crate::marshal::list_from_strs({n})?;"]
        return []

    def call_expr(self) -> str:
        n = self.rust_name
        if self.kind in ("str", "ratio", "complex", "boxed_int", "boxed_float", "boxed_bool",
                         "interface_slice", "str_slice"):
            return f"__{n}.as_ptr() as *mut _"
        if self.kind == "c_str":
            return f"__{n}.as_ptr() as _"
        if self.kind in ("value", "number"):
            return f"crate::value::opt_ref_ptr(&__{n}) as *mut _"
        if self.kind == "opt_str":
            return f"__{n}.as_ref().map_or(std::ptr::null_mut(), |r| r.as_ptr()) as *mut _"
        if self.kind == "interface":
            return f"{n}.as_raw() as *mut _"
        if self.kind == "opt_interface":
            return f"{n}.map_or(std::ptr::null_mut(), |o| o.as_raw() as *mut _)"
        if self.kind == "bool":
            return f"u8::from({n})"
        if self.kind == "enum":
            return f"{n} as u32"
        if self.kind in ("scalar", "intf_id"):
            return n
        raise AssertionError(self.kind)

    def default_call_expr(self) -> str:
        """The FFI expression for this parameter when omitted (base variant)."""
        if self.optional:
            return "std::ptr::null_mut()"
        if self.default in ("true", "True"):
            return "1"
        if self.default in ("false", "False"):
            return "0"
        if self.kind == "enum":
            return f"{self.default}"
        return str(int(self.default, 0))


class Out:
    """One out-parameter with its result conversion strategy."""

    def __init__(self, model: Model, arg: dict, function: dict, is_constructor: bool):
        self.c_name = arg["name"]
        self.rust_name = sanitize_ident(camel_to_snake(arg["name"]))
        t = arg["type"]
        base = t["name"]
        resolved = model.canon(base)
        depth = t.get("pointer_depth", 0)
        self.kind = None
        self.rust_type = None
        self.slot_type = None
        self.is_constructor = is_constructor

        if is_constructor and depth == 2 and resolved in model.opaque:
            # A factory's out-object stays the wrapper type, even for the
            # boxed scalars (BooleanObject::new(true) -> BooleanObject).
            if resolved in EXCLUDED_INTERFACES:
                raise Skip(f"out-parameter of excluded interface {resolved}")
            rust = rust_interface_name(resolved)
            self.kind, self.rust_type = "object_required", rust
            self.iface = rust
            self.slot_type = f"*mut sys::{resolved}"
        elif resolved == "daqString" and depth == 2:
            self.kind, self.rust_type, self.slot_type = "string", "String", "*mut sys::daqString"
        elif base == "daqCharPtr" and depth == 1:
            # Owned by the caller per the toString contract; freed with daqFreeMemory.
            self.kind, self.rust_type, self.slot_type = "char_ptr", "String", "*mut c_char"
        elif base == "daqConstCharPtr" and depth == 1:
            # Borrows storage owned by the object; copied, never freed.
            self.kind, self.rust_type, self.slot_type = "const_char_ptr", "String", "*const c_char"
        elif resolved == "daqBaseObject" and depth == 2:
            self.kind, self.rust_type, self.slot_type = "value", "Value", "*mut sys::daqBaseObject"
        elif resolved == "daqRatio" and depth == 2:
            self.kind, self.rust_type, self.slot_type = "ratio", "Option<Ratio>", "*mut sys::daqRatio"
        elif resolved == "daqComplexNumber" and depth == 2:
            self.kind, self.rust_type, self.slot_type = "complex", "Option<Complex>", "*mut sys::daqComplexNumber"
        elif resolved == "daqInteger" and depth == 2:
            self.kind, self.rust_type, self.slot_type = "boxed_int", "Option<i64>", "*mut sys::daqInteger"
        elif resolved == "daqFloatObject" and depth == 2:
            self.kind, self.rust_type, self.slot_type = "boxed_float", "Option<f64>", "*mut sys::daqFloatObject"
        elif resolved == "daqBoolean" and depth == 2:
            self.kind, self.rust_type, self.slot_type = "boxed_bool", "Option<bool>", "*mut sys::daqBoolean"
        elif resolved == "daqNumber" and depth == 2:
            self.kind, self.rust_type, self.slot_type = "number", "Option<f64>", "*mut sys::daqNumber"
        elif resolved == "daqList" and depth == 2:
            element = t.get("value_type")
            elem_rust, elem_kind = element_rust(element) if element else ("Value", "value")
            if elem_kind == "interface" and "daq" + element in EXCLUDED_INTERFACES:
                raise Skip(f"list of excluded interface {element}")
            self.kind = "list"
            self.elem = (elem_rust, elem_kind)
            self.rust_type = f"Vec<{elem_rust}>"
            self.slot_type = "*mut sys::daqList"
        elif resolved == "daqDict" and depth == 2:
            key = t.get("key_type")
            val = t.get("value_type")
            key_rust, key_kind = element_rust(key) if key else ("Value", "value")
            val_rust, val_kind = element_rust(val) if val else ("Value", "value")
            for e, k in ((key_rust, key_kind), (val_rust, val_kind)):
                if k == "interface" and "daq" + e in EXCLUDED_INTERFACES:
                    raise Skip(f"dict of excluded interface {e}")
            if key_kind in ("string", "int"):
                self.kind = "dict"
                self.elems = ((key_rust, key_kind), (val_rust, val_kind))
                self.rust_type = f"std::collections::HashMap<{key_rust}, {val_rust}>"
            else:
                self.kind = "dict_pairs"
                self.rust_type = "Vec<(Value, Value)>"
            self.slot_type = "*mut sys::daqDict"
        elif resolved in model.opaque and depth == 2:
            if resolved in EXCLUDED_INTERFACES:
                raise Skip(f"out-parameter of excluded interface {resolved}")
            rust = rust_interface_name(resolved)
            if is_constructor:
                self.kind, self.rust_type = "object_required", rust
            else:
                self.kind, self.rust_type = "object", f"Option<{rust}>"
            self.iface = rust
            self.slot_type = f"*mut sys::{resolved}"
        elif resolved in model.enums and depth == 1:
            em = EnumModel(resolved, model.enums[resolved])
            self.kind, self.rust_type, self.slot_type = "enum", em.rust_name, "u32"
        elif resolved == "daqBool" and depth == 1:
            self.kind, self.rust_type, self.slot_type = "bool", "bool", "u8"
        elif resolved in SCALAR_ALIASES and depth == 1 and SCALAR_ALIASES[resolved] in (
                "i64", "u64", "f64", "usize", "u32"):
            self.kind = "scalar"
            self.rust_type = SCALAR_ALIASES[resolved]
            self.slot_type = SCALAR_ALIASES[resolved]
        elif resolved in BUILTIN_TO_RUST and depth == 1:
            self.kind, self.rust_type, self.slot_type = "scalar", BUILTIN_TO_RUST[resolved], BUILTIN_TO_RUST[resolved]
        elif resolved == "daqIntfID" and depth == 1:
            self.kind, self.rust_type, self.slot_type = "intf_id", "crate::IntfID", "crate::IntfID"
        elif resolved == "void" and depth >= 1:
            raise Skip("void* out-parameter")
        else:
            raise Skip(f"unmapped out type {base}{'*' * depth}")

    def slot_init(self) -> str:
        n = self.rust_name
        if self.kind == "const_char_ptr":
            return f"let mut __{n}: {self.slot_type} = std::ptr::null();"
        if self.kind in ("scalar",):
            return f"let mut __{n}: {self.slot_type} = Default::default();"
        if self.kind in ("enum", "bool"):
            return f"let mut __{n}: {self.slot_type} = 0;"
        if self.kind == "intf_id":
            return f"let mut __{n} = crate::IntfID {{ Data1: 0, Data2: 0, Data3: 0, Data4: 0 }};"
        return f"let mut __{n}: {self.slot_type} = std::ptr::null_mut();"

    def call_expr(self) -> str:
        n = self.rust_name
        if self.kind == "enum":
            return f"&mut __{n}"
        return f"&mut __{n}"

    def result_expr(self, op: str) -> str:
        n = self.rust_name
        if self.kind == "string":
            return f"unsafe {{ crate::marshal::take_string(__{n}) }}"
        if self.kind == "char_ptr":
            return f"unsafe {{ crate::marshal::take_char_ptr(__{n}) }}"
        if self.kind == "const_char_ptr":
            return f"unsafe {{ crate::marshal::copy_const_char_ptr(__{n}) }}"
        if self.kind == "value":
            return f"unsafe {{ crate::value::take_value(__{n}, \"{op}\") }}?"
        if self.kind == "ratio":
            return f"unsafe {{ crate::value::take_ratio(__{n}, \"{op}\") }}?"
        if self.kind == "complex":
            return f"unsafe {{ crate::value::take_complex(__{n}, \"{op}\") }}?"
        if self.kind == "boxed_int":
            return f"unsafe {{ crate::value::take_boxed_int(__{n}, \"{op}\") }}?"
        if self.kind == "boxed_float":
            return f"unsafe {{ crate::value::take_boxed_float(__{n}, \"{op}\") }}?"
        if self.kind == "boxed_bool":
            return f"unsafe {{ crate::value::take_boxed_bool(__{n}, \"{op}\") }}?"
        if self.kind == "number":
            return f"unsafe {{ crate::value::take_number(__{n}, \"{op}\") }}?"
        if self.kind == "list":
            elem_rust, _ = self.elem
            return f"unsafe {{ crate::marshal::take_list::<{elem_rust}>(__{n} as *mut _, \"{op}\") }}?"
        if self.kind == "dict":
            (k, _), (v, _) = self.elems
            return f"unsafe {{ crate::marshal::take_dict::<{k}, {v}>(__{n} as *mut _, \"{op}\") }}?"
        if self.kind == "dict_pairs":
            return f"unsafe {{ crate::marshal::take_dict_pairs(__{n} as *mut _, \"{op}\") }}?"
        if self.kind == "object":
            return f"unsafe {{ crate::marshal::take_object::<{self.iface}>(__{n} as *mut _) }}"
        if self.kind == "object_required":
            return f"unsafe {{ crate::marshal::require_object::<{self.iface}>(__{n} as *mut _, \"{op}\") }}?"
        if self.kind == "enum":
            return f"crate::marshal::enum_out({self.rust_type}::from_raw(__{n}), \"{op}\")?"
        if self.kind == "bool":
            return f"__{n} != 0"
        if self.kind in ("scalar", "intf_id"):
            return f"__{n}"
        raise AssertionError(self.kind)


# ---------------------------------------------------------------------------
# High-level function emission
# ---------------------------------------------------------------------------

def is_out_param(model: Model, function: dict, arg: dict) -> bool:
    """The parameter-mode classification: a non-pointer is
    an input; a single pointer to an interface, callback, or daqBaseObject is
    the object itself passed in; void* is a raw pass-through buffer; anything
    else written through a pointer is an out-slot."""
    t = arg["type"]
    depth = t.get("pointer_depth", 0)
    if depth == 0:
        return False
    base = model.canon(t["name"])
    if depth == 1 and (base in model.opaque or base in model.callbacks or base == "daqBaseObject"):
        return False
    if base == "void":
        return False
    if depth == 1 and base == "char" and t["name"] not in ("daqCharPtr", "daqConstCharPtr"):
        # a raw `const char*` argument (spelled without the alias) is an input
        return False
    return True


def receiver_of(function: dict) -> str:
    return function["name"].partition("_")[0]


def method_stem(function: dict) -> str:
    return function["name"].partition("_")[2]


def classify(model: Model, function: dict) -> str:
    stem = method_stem(function)
    args = function.get("arguments", [])
    outs = [a for a in args if is_out_param(model, function, a)]
    has_self = bool(args) and args[0]["name"] == "self"
    if stem.startswith("create") and not has_self and len(outs) == 1 and outs[0]["name"] == "obj":
        return "constructor"
    if stem.startswith("get"):
        return "reader"
    if stem.startswith("set") and len(args) > (1 if has_self else 0) and not outs:
        return "writer"
    return "method"


def strip_token_run(body_tokens: list[str], run_tokens: list[str]) -> list[str]:
    for i in range(len(body_tokens) - len(run_tokens) + 1):
        if body_tokens[i:i + len(run_tokens)] == run_tokens:
            return body_tokens[:i] + body_tokens[i + len(run_tokens):]
    return body_tokens


def constructor_rust_name(function: dict, receiver: str) -> str:
    """Associated-function name for a factory constructor: `new` for the
    canonical create<Receiver>, `from_builder` for create<Receiver>FromBuilder,
    and the create-stem minus the receiver's tokens otherwise
    (createIntProperty on daqProperty -> `int`)."""
    if function["name"] in CONSTRUCTOR_NAME_OVERRIDES:
        return CONSTRUCTOR_NAME_OVERRIDES[function["name"]]
    stem = method_stem(function)
    body = camel_to_snake(stem[len("create"):])
    receiver_snake = camel_to_snake(receiver[3:])
    if body == receiver_snake:
        return "new"
    if body == receiver_snake + "_from_builder":
        return "from_builder"
    tokens = strip_token_run(body.split("_"), receiver_snake.split("_"))
    name = "_".join(tokens)
    if not name:
        raise ValueError(f"empty constructor name for {function['name']}")
    return sanitize_ident(name)


# Method names that would shadow a standard trait method on the wrapper
# (an inherent `clone` would hijack every `Clone::clone` call).
METHOD_RENAMES = {"clone": "clone_object"}


def method_rust_name(function: dict, kind: str) -> str:
    stem = method_stem(function)
    name = camel_to_snake(stem)
    if kind == "reader" and name.startswith("get_"):
        name = name[len("get_"):]
    name = METHOD_RENAMES.get(name, name)
    return sanitize_ident(name)


def emit_function(model: Model, function: dict, skips: list[tuple[str, str]]) -> list[str] | None:
    """Emit one method / associated function, or record a skip and return None."""
    c_name = function["name"]
    receiver = receiver_of(function)
    kind = classify(model, function)

    args = function.get("arguments", [])
    has_self = bool(args) and args[0]["name"] == "self"
    ret = function["return_type"]
    returns_err = ret["name"] == "daqErrCode" and ret.get("pointer_depth", 0) == 0
    returns_void = ret["name"] == "void" and ret.get("pointer_depth", 0) == 0
    if not returns_err and not returns_void:
        skips.append((c_name, f"returns {ret['name']}"))
        return None

    try:
        params: list[Param] = []
        outs: list[Out] = []
        for arg in args:
            if has_self and arg is args[0]:
                continue
            if is_out_param(model, function, arg):
                outs.append(Out(model, arg, function, kind == "constructor"))
            else:
                params.append(Param(model, arg, function))
    except Skip as reason:
        skips.append((c_name, str(reason)))
        return None

    if kind == "constructor":
        rust_name = constructor_rust_name(function, receiver)
        outs = [o for o in outs]  # single 'obj'
    elif kind == "writer":
        rust_name = sanitize_ident(camel_to_snake(method_stem(function)))
    else:
        rust_name = method_rust_name(function, kind)

    # Trailing parameters with C default values also get a convenience variant
    # without them (mirroring the C++ API's default arguments).
    def default_expressible(p: Param) -> bool:
        if p.optional or p.default in ("true", "True", "false", "False"):
            return True
        try:
            int(p.default, 0)
            return p.kind in ("scalar", "bool")
        except (TypeError, ValueError):
            return False

    defaulted: list[Param] = []
    required = list(params)
    while required and required[-1].default is not None and default_expressible(required[-1]):
        defaulted.insert(0, required.pop())
    if any(p.default is not None for p in required):
        defaulted = []  # defaults not trailing (or not expressible): single full method
        required = list(params)

    op = c_name
    lines: list[str] = []

    def emit_variant(name: str, use_params: list[Param], omitted: list[Param]) -> None:
        sig_params = ", ".join(p.signature() for p in use_params)
        self_part = "&self" + (", " if sig_params else "") if has_self else ""
        if not has_self and sig_params == "":
            sig = ""
        else:
            sig = sig_params
        if len(outs) == 0:
            ret_type = "()"
        elif len(outs) == 1:
            ret_type = outs[0].rust_type
        else:
            ret_type = "(" + ", ".join(o.rust_type for o in outs) + ")"
        lines.extend(doc_lines(function.get("docstring", ""), "    "))
        lines.append(f"    /// Calls the openDAQ C function `{c_name}()`.")
        lines.append(f"    pub fn {name}({self_part}{sig}) -> Result<{ret_type}> {{")
        for p in use_params:
            for stmt in p.prologue():
                lines.append(f"        {stmt}")
        for o in outs:
            lines.append(f"        {o.slot_init()}")
        call_args: list[str] = []
        for arg in args:
            if has_self and arg is args[0]:
                call_args.append("self.as_raw() as *mut _")
                continue
            param = next((p for p in use_params if p.c_name == arg["name"]), None)
            if param is not None:
                call_args.append(param.call_expr())
                continue
            omitted_param = next((p for p in omitted if p.c_name == arg["name"]), None)
            if omitted_param is not None:
                call_args.append(omitted_param.default_call_expr())
                continue
            out = next(o for o in outs if o.c_name == arg["name"])
            call_args.append(out.call_expr())
        joined = ", ".join(call_args)
        if returns_err:
            lines.append(
                f"        let __code = unsafe {{ (crate::sys::api().{c_name})({joined}) }};"
            )
            lines.append(f"        check(__code, \"{op}\")?;")
        else:
            lines.append(f"        unsafe {{ (crate::sys::api().{c_name})({joined}) }};")
        if len(outs) == 0:
            lines.append("        Ok(())")
        elif len(outs) == 1:
            lines.append(f"        Ok({outs[0].result_expr(op)})")
        else:
            exprs = ", ".join(o.result_expr(op) for o in outs)
            lines.append(f"        Ok(({exprs}))")
        lines.append("    }")
        lines.append("")

    emit_variant(rust_name, required, defaulted)
    if defaulted:
        emit_variant(f"{rust_name}_with", required + defaulted, [])
    return lines


# ---------------------------------------------------------------------------
# High-level module emission
# ---------------------------------------------------------------------------

def interface_module(model: Model, c_name: str) -> str:
    record = model.typedefs.get(c_name)
    if record is not None:
        return model.module_of(record["source_file"])
    source = model.interface_sources.get(c_name)
    return model.module_of(source) if source else "coretypes"


def emit_high_level(model: Model, output_dir: Path) -> None:
    skips: list[tuple[str, str]] = []

    # Which opaque types become generated interface structs.
    interfaces = sorted(
        name for name in model.opaque
        if name not in model.opaque_aliases
        and name not in EXCLUDED_INTERFACES
        and name != "daqBaseObject"
    )

    def parent_of(c_name: str) -> str:
        parent = model.interface_parent.get(c_name, "daqBaseObject")
        if parent in EXCLUDED_INTERFACES or parent not in model.opaque:
            if parent != "daqBaseObject" and parent not in model.opaque:
                pass
            parent = "daqBaseObject"
        return parent

    modules: dict[str, list[str]] = defaultdict(list)

    for c_name in interfaces:
        rust = rust_interface_name(c_name)
        parent_c = parent_of(c_name)
        parent_rust = "BaseObject" if parent_c == "daqBaseObject" else rust_interface_name(parent_c)
        module = modules[interface_module(model, c_name)]

        module.extend(doc_lines(model.interface_docs.get(c_name, ""), ""))
        module.append(f"/// Wrapper over the openDAQ `{c_name}` interface.")
        module.append("#[repr(transparent)]")
        module.append("#[derive(Clone, Debug)]")
        module.append(f"pub struct {rust}(pub(crate) {parent_rust});")
        module.append("")
        module.append(f"impl std::ops::Deref for {rust} {{")
        module.append(f"    type Target = {parent_rust};")
        module.append(f"    fn deref(&self) -> &{parent_rust} {{ &self.0 }}")
        module.append("}")
        module.append(f"impl crate::sealed::Sealed for {rust} {{}}")
        module.append(f"unsafe impl Interface for {rust} {{")
        module.append(f"    const NAME: &'static str = \"{c_name}\";")
        if c_name in model.has_interface_id:
            module.append("    fn interface_id() -> Option<crate::IntfID> {")
            module.append("        let mut id = crate::IntfID { Data1: 0, Data2: 0, Data3: 0, Data4: 0 };")
            module.append(f"        unsafe {{ (crate::sys::api().{c_name}_getInterfaceId)(&mut id) }};")
            module.append("        Some(id)")
            module.append("    }")
        else:
            module.append("    fn interface_id() -> Option<crate::IntfID> { None }")
        module.append("    unsafe fn from_raw(ptr: *mut std::ffi::c_void) -> Option<Self> {")
        module.append(f"        Ref::from_owned(ptr).map(Self::__from_ref)")
        module.append("    }")
        module.append("    fn as_base_object(&self) -> &BaseObject { &self.0 }")
        module.append("}")
        module.append(f"impl {rust} {{")
        module.append("    #[doc(hidden)]")
        if parent_rust == "BaseObject":
            module.append(f"    pub(crate) fn __from_ref(r: Ref) -> Self {{ {rust}(BaseObject(r)) }}")
        else:
            module.append(
                f"    pub(crate) fn __from_ref(r: Ref) -> Self {{ {rust}({parent_rust}::__from_ref(r)) }}"
            )
        module.append("}")
        module.append(f"impl std::fmt::Display for {rust} {{")
        module.append("    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {")
        module.append("        std::fmt::Display::fmt(self.as_base_object(), f)")
        module.append("    }")
        module.append("}")
        module.append(f"impl From<&{rust}> for Value {{")
        module.append(f"    fn from(value: &{rust}) -> Value {{ Value::Object(value.to_base_object()) }}")
        module.append("}")
        module.append(f"impl From<{rust}> for Value {{")
        module.append(f"    fn from(value: {rust}) -> Value {{ Value::Object(value.to_base_object()) }}")
        module.append("}")
        module.append(f"impl crate::value::FromDaqOwned for {rust} {{")
        module.append("    unsafe fn from_daq_owned(ptr: *mut std::ffi::c_void, op: &'static str) -> Result<Self> {")
        module.append("        crate::value::cast_owned(ptr, op)")
        module.append("    }")
        module.append("}")
        module.append("")

    # Methods grouped per interface.
    impls: dict[str, list[str]] = defaultdict(list)
    for function in model.functions:
        c_name = function["name"]
        receiver = receiver_of(function)
        if "_" not in c_name or receiver in MANUAL_RECEIVERS:
            continue
        if receiver in EXCLUDED_INTERFACES or receiver in model.opaque_aliases:
            continue
        if receiver not in model.opaque:
            skips.append((c_name, f"receiver {receiver} is not an interface"))
            continue
        if c_name in MANUAL_FUNCTIONS:
            skips.append((c_name, "hand-written in the crate"))
            continue
        if c_name in IN_OUT_FUNCTIONS:
            skips.append((c_name, "in-out parameter; hand-written"))
            continue
        if method_stem(function) == "getInterfaceId":
            continue
        if classify(model, function) == "constructor" and receiver in (
                "daqInstance", "daqProcedure", "daqFunction", "daqEventHandler"):
            skips.append((c_name, "manual constructor"))
            continue
        body = emit_function(model, function, skips)
        if body is not None:
            impls[receiver].extend(body)

    for receiver, body in impls.items():
        rust = rust_interface_name(receiver)
        module = modules[interface_module(model, receiver)]
        module.append(f"impl {rust} {{")
        module.extend(body)
        module.append("}")
        module.append("")

    generated_dir = output_dir / "generated"
    generated_dir.mkdir(parents=True, exist_ok=True)
    for old in generated_dir.glob("*.rs"):
        old.unlink()

    prelude = [
        HEADER,
        "// Mechanical output: lint findings here are the generator's business.",
        "#![allow(clippy::all, unused_imports, rustdoc::bare_urls)]",
        "",
        "use crate::error::{check, Error, Result};",
        "use crate::object::{BaseObject, Interface, Ref};",
        "use crate::sys;",
        "use crate::value::{Complex, Ratio, Value};",
        "use crate::generated::*;",
        "use std::ffi::c_char;",
        "",
    ]

    module_names = sorted(modules)
    for name in module_names:
        content = "\n".join(prelude + modules[name]) + "\n"
        (generated_dir / f"{name}.rs").write_text(content, encoding="utf-8", newline="\n")

    enum_names = sorted(
        EnumModel(n, t).rust_name for n, t in model.enums.items()
    )
    mod_lines = [HEADER]
    for name in module_names:
        mod_lines.append(f"mod {name};")
    mod_lines.append("")
    for name in module_names:
        mod_lines.append(f"pub use {name}::*;")
    mod_lines.append("")
    mod_lines.append("pub use crate::sys::{" + ", ".join(enum_names) + "};")
    mod_lines.append("")
    (generated_dir / "mod.rs").write_text("\n".join(mod_lines), encoding="utf-8", newline="\n")

    # Skip report.
    print(f"generated {len(model.functions)} C functions across {len(module_names)} modules")
    print(f"skipped {len(skips)} functions:")
    for name, reason in sorted(skips):
        print(f"  {name}: {reason}")


# ---------------------------------------------------------------------------
# main
# ---------------------------------------------------------------------------

def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--opendaq-repo", type=Path, required=True)
    ap.add_argument("--output-dir", type=Path,
                    default=Path(__file__).resolve().parents[1] / "src")
    args = ap.parse_args()

    records = parse_records(args.opendaq_repo)
    model = Model(records)

    sys_dir = args.output_dir / "sys"
    sys_dir.mkdir(parents=True, exist_ok=True)
    (sys_dir / "generated.rs").write_text(emit_sys(model), encoding="utf-8", newline="\n")

    emit_high_level(model, args.output_dir)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
