For Bela-Ciao VM, the right audit target in this repo is the latest static PTB
executor:

- Entry routing: `sui-execution/latest/sui-adapter/src/execution_engine.rs:606`
- New VM entrypoint:
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/mod.rs:38`
- Runtime construction: `sui-execution/latest/sui-adapter/src/adapter.rs:38`
- Executor selection: `sui-execution/src/lib.rs:32`

The key architectural fact is that `latest` no longer keeps the legacy PTB
executor branch. In `v3`, PTB execution was gated by
`enable_ptb_execution_v2()` and could still fall back to the old path at
`sui-execution/v3/sui-adapter/src/programmable_transactions/execution.rs:84`.
In `latest`, `TransactionKind::ProgrammableTransaction` goes directly to
`SPT::execute` at
`sui-execution/latest/sui-adapter/src/execution_engine.rs:716`.

So the Bela-Ciao audit surface is:

- `loading/`: raw PTB to internal AST translation
- `typing/`: type translation and verifier logic
- `linkage/`: package/type resolution and VM linkage context
- `execution/`: interpreter semantics
- `metering/`: translation/execution charging boundaries
- `env.rs`: host capabilities exposed to the new executor

The critical trust boundary inside the new VM is:

`ProgrammableTransaction -> linkage resolution -> translated typed IR -> interpreter execution`

Concretely, the first files to deep-read are:

- `sui-execution/latest/sui-adapter/src/static_programmable_transactions/loading/translate.rs`
- `sui-execution/latest/sui-adapter/src/static_programmable_transactions/typing/translate.rs`
- `sui-execution/latest/sui-adapter/src/static_programmable_transactions/typing/verify/mod.rs`
- `sui-execution/latest/sui-adapter/src/static_programmable_transactions/linkage/analysis.rs`
- `sui-execution/latest/sui-adapter/src/static_programmable_transactions/execution/interpreter.rs`
- `sui-execution/latest/sui-adapter/src/static_programmable_transactions/execution/context.rs`

## Official Docs Context Crosswalk

The official docs describe Bela-Ciao as introducing per-package caching,
reworking type storage and resolution, changing how execution is handled across
packages, and changing how the interpreter processes instructions. The codebase
supports those claims as follows:

### 1. Per-package caching reduces load work and improves runtime reuse

- `CachedPackageStore` is explicitly documented as a package-loading layer that
  uses `MoveRuntime` to fetch and cache packages:
  `sui-execution/latest/sui-adapter/src/data_store/cached_package_store.rs:18`
- The cache lookup path first checks packages published in the current
  transaction, then falls back to runtime-backed resolution and caching:
  `sui-execution/latest/sui-adapter/src/data_store/cached_package_store.rs:61`
- The static PTB executor wraps its package store in `CachedPackageStore`
  before linkage and execution:
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/mod.rs:52`

Why this matters for audit context:

- Package loading is no longer just a raw storage read path.
- Runtime package cache behavior is now part of correctness and performance.
- Newly published packages and cached historical packages are intentionally
  handled through different code paths.

### 2. Type storage and type resolution were reworked

- The new typed PTB AST stores typed inputs and typed command results directly:
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/typing/ast.rs:20`
- `ObjectInput`, `PureInput`, `ReceivingInput`, and `WithdrawalInput` all carry
  explicit `Type` metadata:
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/typing/ast.rs:42`
- Commands such as `Publish` and `Upgrade` embed `ResolvedLinkage` directly in
  the typed AST:
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/typing/ast.rs:124`
- The linkage analyzer computes input-type resolution linkage from both PTB
  inputs and command structure before execution:
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/linkage/analysis.rs:79`
- Type-defining package IDs are extracted from on-chain object types and added
  into the resolution table:
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/linkage/analysis.rs:251`
- Package-level type origins are used to resolve a type back to its defining
  package:
  `sui-execution/latest/sui-adapter/src/data_store/cached_package_store.rs:93`

Why this matters for audit context:

- Type information is promoted into explicit intermediate representations.
- Type resolution is no longer an incidental byproduct of legacy execution.
- Linkage and defining-package identity become first-class audit targets.

### 3. Execution across packages is handled through explicit linkage analysis

- Before translation or execution, the VM computes an executable linkage for
  the transaction inputs:
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/mod.rs:53`
- A `resolution_vm` is then created from the base runtime plus that computed
  linkage context:
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/mod.rs:59`
- Call linkage resolves the package, inspects function visibility, and applies
  exact vs at-least version constraints across dependencies:
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/linkage/analysis.rs:93`
- Input objects contribute their defining type package addresses into linkage
  analysis:
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/linkage/analysis.rs:233`

Why this matters for audit context:

- Cross-package execution is not just "invoke package X". It depends on a
  constructed linkage context.
- Visibility, dependency version constraints, and type-defining IDs all affect
  what code is considered executable.
- Package resolution bugs can now alter execution context even before the first
  instruction is interpreted.

### 4. The interpreter now executes a typed command IR directly

- After loading and typing, the static executor hands a typed transaction AST to
  the interpreter:
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/mod.rs:91`
- `execute_inner` destructures the typed transaction into typed inputs and a
  typed command list:
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/execution/interpreter.rs:73`
- The interpreter processes commands one by one and records per-command timing:
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/execution/interpreter.rs:99`
- `execute_command` switches on the typed command enum
  `T::Command__::{MoveCall, TransferObjects, SplitCoins, ...}`:
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/execution/interpreter.rs:143`

Why this matters for audit context:

- The interpreter surface is now the typed AST, not the raw PTB shape.
- Command semantics are centralized in a dedicated interpreter over explicit IR.
- Auditing instruction handling now means auditing typed command dispatch plus
  context mutation rules.

### 5. "Groundwork for future Move features" is an architectural inference supported by the codebase

This is an inference from the implementation, not a direct statement found in
the code comments.

- The pipeline is split into explicit phases:
  `loading`, `typing`, `linkage`, `metering`, and `execution` in
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/mod.rs:30`
- The typed AST has explicit nodes for typed inputs, borrow/copy/freeze usage,
  result types, shared-object consumption, and package-linked publish/upgrade
  commands:
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/typing/ast.rs:95`
- Linkage is computed separately from execution and then used to construct a
  specialized resolution VM:
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/mod.rs:53`

Why this matters for audit context:

- The architecture is more modular than the legacy single-path PTB executor.
- New language features can plausibly be added at the loading, typing, linkage,
  or interpreter layers without overloading one monolithic execution path.
- For audit purposes, extensibility boundaries are now visible and should be
  reviewed as security boundaries too.

## Audit Checklist

This checklist is for context building and structured review of the Bela-Ciao
execution path. It is organized around the same five feature areas above.

### 1. Per-package caching

Primary code:

- `sui-execution/latest/sui-adapter/src/data_store/cached_package_store.rs:18`
- `sui-execution/latest/sui-adapter/src/static_programmable_transactions/mod.rs:52`
- `sui-execution/latest/sui-adapter/src/static_programmable_transactions/execution/context.rs:152`

Review questions:

- Does package lookup preserve the intended priority order of
  `newly-published package -> runtime cache -> backing store`?
- Can a package published in the current transaction be accidentally inserted
  into or shadowed by the runtime cache?
- Are cache hits keyed strongly enough to avoid reusing the wrong package or
  wrong linkage context?
- Does per-package caching preserve deterministic behavior across validators,
  not just local performance?
- Does cache reuse ever cross a trust boundary where linkage or native
  extensions should have forced a fresh VM/package resolution?

Key invariants to verify:

- New packages are returned directly rather than being silently cached as
  historical runtime state.
- Runtime package caching only occurs through the intended `MoveRuntime`
  resolution path.
- VM reuse in `Context` is keyed by linkage hash, not only by package ID.

Concrete places to inspect:

- `CachedPackageStore::fetch_package`
  `sui-execution/latest/sui-adapter/src/data_store/cached_package_store.rs:61`
- `resolve_and_cache_package`
  `sui-execution/latest/sui-adapter/src/data_store/cached_package_store.rs:72`
- `with_vm!` executable VM cache reuse
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/execution/context.rs:152`

### 2. Type storage and type resolution

Primary code:

- `sui-execution/latest/sui-adapter/src/static_programmable_transactions/typing/ast.rs:20`
- `sui-execution/latest/sui-adapter/src/static_programmable_transactions/linkage/analysis.rs:79`
- `sui-execution/latest/sui-adapter/src/static_programmable_transactions/env.rs:145`
- `sui-execution/latest/sui-adapter/src/data_store/cached_package_store.rs:93`

Review questions:

- Are all user-visible and runtime-relevant input categories assigned explicit
  types before execution?
- Do type-defining package IDs come from trustworthy sources at each step
  of resolution?
- Can type resolution diverge between input-type resolution and call-time
  linkage resolution?
- Are layout derivation and type-tag conversion using the same defining-package
  assumptions as execution?
- Can aliasing or upgraded packages cause the same nominal type to resolve to
  different defining IDs across phases?

Key invariants to verify:

- Every object, pure input, receiving input, and withdrawal input carries
  explicit type information in the typed AST.
- Type origins come from package metadata, not ad hoc name matching.
- Input-type layout resolution uses the dedicated resolution VM and preserves
  linkage-aware error handling.

Concrete places to inspect:

- Typed inputs in AST
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/typing/ast.rs:42`
- Input-type linkage computation
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/linkage/analysis.rs:208`
- Layout derivation
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/env.rs:145`
- Type origin lookup
  `sui-execution/latest/sui-adapter/src/data_store/cached_package_store.rs:103`

### 3. Execution across packages

Primary code:

- `sui-execution/latest/sui-adapter/src/static_programmable_transactions/mod.rs:53`
- `sui-execution/latest/sui-adapter/src/static_programmable_transactions/linkage/analysis.rs:93`
- `sui-execution/latest/sui-adapter/src/static_programmable_transactions/env.rs:193`
- `sui-execution/latest/sui-adapter/src/static_programmable_transactions/loading/translate.rs:197`

Review questions:

- Is linkage computed from all relevant transaction inputs and commands before
  execution begins?
- Are visibility rules correctly mapped into exact vs at-least version
  constraints?
- Can publication/upgrade commands build incomplete or over-permissive linkage
  contexts?
- Does function loading use the same linkage assumptions as later execution in
  the interpreter?
- Are package IDs, original IDs, and module IDs translated consistently across
  upgrades and cross-package calls?

Key invariants to verify:

- Call linkage includes transitive dependencies required by both the callee and
  type arguments.
- Input object types contribute defining-package addresses into resolution.
- Publication and upgrade commands compute explicit publication linkage rather
  than inheriting arbitrary ambient execution linkage.

Concrete places to inspect:

- PTB-wide linkage bootstrap
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/mod.rs:53`
- Function visibility and version constraints
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/linkage/analysis.rs:142`
- Function loading through computed linkage
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/env.rs:203`
- Publish/upgrade translation
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/loading/translate.rs:229`

### 4. Interpreter instruction processing

Primary code:

- `sui-execution/latest/sui-adapter/src/static_programmable_transactions/execution/interpreter.rs:61`
- `sui-execution/latest/sui-adapter/src/static_programmable_transactions/execution/context.rs:234`
- `sui-execution/latest/sui-adapter/src/static_programmable_transactions/typing/verify/mod.rs:17`

Review questions:

- Does every typed command variant have one execution path with well-defined
  input consumption, result production, and gas effects?
- Are pre-execution verifier passes sufficient to prevent interpreter misuse of
  borrowed, copied, frozen, or dropped values?
- Does command failure preserve the intended partial-execution semantics and
  diagnostics, including command index attribution?
- Are runtime-loaded child objects, wrapped-object metadata, and generated IDs
  always recorded consistently on both success and failure paths?
- Can command dispatch or argument extraction violate stack-height, aliasing, or
  ownership assumptions?

Key invariants to verify:

- Stack height begins at zero for each command.
- The interpreter executes typed commands, not raw PTB commands.
- Failure paths still persist runtime object-loading metadata needed for replay
  and accounting.
- Verifier passes run before interpretation and cover input arguments, move
  functions, memory safety, drop safety, and private entry arguments.

Concrete places to inspect:

- Interpreter entry
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/execution/interpreter.rs:61`
- Command loop and abort handling
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/execution/interpreter.rs:99`
- Command dispatch
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/execution/interpreter.rs:143`
- Verification passes
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/typing/verify/mod.rs:17`
- Runtime state container and VM cache
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/execution/context.rs:234`

### 5. Extensibility and future-feature groundwork

Primary code:

- `sui-execution/latest/sui-adapter/src/static_programmable_transactions/mod.rs:30`
- `sui-execution/latest/sui-adapter/src/static_programmable_transactions/loading/translate.rs:21`
- `sui-execution/latest/sui-adapter/src/static_programmable_transactions/typing/verify/mod.rs:17`
- `sui-execution/latest/sui-adapter/src/static_programmable_transactions/env.rs:4`

Review questions:

- Are phase boundaries clean enough that new features can be inserted without
  bypassing existing validation or metering?
- Do new feature hooks naturally belong to loading, typing, linkage, or
  execution, or would they need unsafe cross-phase shortcuts?
- Are protocol flags checked at the earliest correct phase, rather than being
  deferred until execution?
- Does the environment object centralize shared facilities in a way that avoids
  duplicated resolution logic across future feature additions?
- Are there hidden legacy assumptions in the new pipeline that would break when
  new command kinds, type forms, or linkage rules are added?

Key invariants to verify:

- Translation, typing, and execution are distinct phases with explicit inputs
  and outputs.
- Shared facilities such as protocol config, linkage analysis, package store,
  and type-resolution VM are centralized in `Env`.
- Protocol-gated behaviors are asserted during translation or verification when
  possible, not silently tolerated until late execution.

Concrete places to inspect:

- Phase structure
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/mod.rs:30`
- Pre-translation metering and protocol assertions
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/loading/translate.rs:31`
- Shared environment design
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/env.rs:4`

## Suggested Audit Order

- Start with `static_programmable_transactions/mod.rs` to anchor the full
  phase pipeline.
- Move to `data_store/cached_package_store.rs` and `linkage/analysis.rs` to
  understand package identity and cross-package execution context.
- Read `typing/ast.rs`, `typing/translate.rs`, and `typing/verify/*` to build
  the typed IR and safety model.
- Read `execution/context.rs` before `execution/interpreter.rs` because the
  interpreter’s safety properties depend heavily on context and VM reuse rules.
- Return to `execution_engine.rs` last to reconnect Bela-Ciao internals back to
  the authority-level execution path.
