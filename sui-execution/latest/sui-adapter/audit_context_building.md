# Audit Context Building Notes

This note applies the Trail of Bits `audit-context-building` workflow to the `sui-execution/latest` workspace.

Scope for this pass:
- Initial bottom-up orientation of the workspace.
- Ultra-granular micro-analysis of the static PTB execution handoff:
  - `sui-adapter/src/static_programmable_transactions/mod.rs::execute`
  - `sui-adapter/src/static_programmable_transactions/execution/context.rs::Context::new`
  - `sui-adapter/src/static_programmable_transactions/execution/interpreter.rs::execute_inner`

This document is intentionally context-only. It does not make vulnerability claims or remediation recommendations.

## Phase 1: Initial Orientation

### Major modules/files

- `sui-adapter`
  - Outer transaction execution, gas integration, temporary store/journal handling, and the static programmable transaction executor.
- `sui-move-natives`
  - Native function registration and object-runtime support used by Move execution.
- `sui-verifier`
  - Move bytecode policy and verifier checks before execution.

### Obvious entrypoints

- Outer transaction execution enters at [`sui-adapter/src/execution_engine.rs:119`](/home/vielite/hackenProof/sui/sui-execution/latest/sui-adapter/src/execution_engine.rs:119).
- Static PTB execution enters at [`sui-adapter/src/static_programmable_transactions/mod.rs:38`](/home/vielite/hackenProof/sui/sui-execution/latest/sui-adapter/src/static_programmable_transactions/mod.rs:38).
- Typed PTB execution enters the interpreter at [`sui-adapter/src/static_programmable_transactions/execution/interpreter.rs:29`](/home/vielite/hackenProof/sui/sui-execution/latest/sui-adapter/src/static_programmable_transactions/execution/interpreter.rs:29).
- Runtime locals/native-extension materialization begins at [`sui-adapter/src/static_programmable_transactions/execution/context.rs:301`](/home/vielite/hackenProof/sui/sui-execution/latest/sui-adapter/src/static_programmable_transactions/execution/context.rs:301).

### Likely actors

- Transaction signer and optional gas sponsor, derived in [`sui-adapter/src/execution_engine.rs:162`](/home/vielite/hackenProof/sui/sui-execution/latest/sui-adapter/src/execution_engine.rs:162).
- The backing store / package store, which provide code and object state to the executor.
- The `GasCharger`, which constrains execution and materializes gas-payment semantics.
- The typed PTB translator/verifier, which converts raw user PTB data into an internal typed IR before interpretation.
- Move-native extensions, especially `ObjectRuntime`, which track runtime object behavior after `Context::new`.

### Important state structures

- `TemporaryStore` in the outer engine holds transaction-local object changes before effects are built; it is constructed at [`sui-adapter/src/execution_engine.rs:153`](/home/vielite/hackenProof/sui/sui-execution/latest/sui-adapter/src/execution_engine.rs:153).
- `TxContext` is created once per transaction at [`sui-adapter/src/execution_engine.rs:181`](/home/vielite/hackenProof/sui/sui-execution/latest/sui-adapter/src/execution_engine.rs:181) and then threaded through translation and execution.
- `Context.locations` in `Context::new` contains the interpreter-visible local model: gas, object inputs, withdrawals, pure inputs, receiving inputs, and command results at [`sui-adapter/src/static_programmable_transactions/execution/context.rs:418`](/home/vielite/hackenProof/sui/sui-execution/latest/sui-adapter/src/static_programmable_transactions/execution/context.rs:418).
- `ObjectRuntime` is seeded through native extensions in [`sui-adapter/src/static_programmable_transactions/execution/context.rs:384`](/home/vielite/hackenProof/sui/sui-execution/latest/sui-adapter/src/static_programmable_transactions/execution/context.rs:384).

### Preliminary system shape

Observed execution pipeline:

1. Outer engine builds transaction-scoped state (`TemporaryStore`, `GasCharger`, `TxContext`) in [`sui-adapter/src/execution_engine.rs:153`](/home/vielite/hackenProof/sui/sui-execution/latest/sui-adapter/src/execution_engine.rs:153) through [`sui-adapter/src/execution_engine.rs:192`](/home/vielite/hackenProof/sui/sui-execution/latest/sui-adapter/src/execution_engine.rs:192).
2. Static PTB execution resolves package linkage, translates raw PTB, type-checks it, and only then calls the interpreter in [`sui-adapter/src/static_programmable_transactions/mod.rs:51`](/home/vielite/hackenProof/sui/sui-execution/latest/sui-adapter/src/static_programmable_transactions/mod.rs:51) through [`sui-adapter/src/static_programmable_transactions/mod.rs:99`](/home/vielite/hackenProof/sui/sui-execution/latest/sui-adapter/src/static_programmable_transactions/mod.rs:99).
3. `Context::new` turns typed inputs into concrete runtime locals and native-extension state in [`sui-adapter/src/static_programmable_transactions/execution/context.rs:317`](/home/vielite/hackenProof/sui/sui-execution/latest/sui-adapter/src/static_programmable_transactions/execution/context.rs:317) through [`sui-adapter/src/static_programmable_transactions/execution/context.rs:430`](/home/vielite/hackenProof/sui/sui-execution/latest/sui-adapter/src/static_programmable_transactions/execution/context.rs:430).
4. The interpreter runs each typed command and commits the finished runtime state back to the `ExecutionState` in [`sui-adapter/src/static_programmable_transactions/execution/interpreter.rs:99`](/home/vielite/hackenProof/sui/sui-execution/latest/sui-adapter/src/static_programmable_transactions/execution/interpreter.rs:99) through [`sui-adapter/src/static_programmable_transactions/execution/interpreter.rs:140`](/home/vielite/hackenProof/sui/sui-execution/latest/sui-adapter/src/static_programmable_transactions/execution/interpreter.rs:140).

## Phase 2: Ultra-Granular Function Analysis

## Function: `static_programmable_transactions::execute`
Source: [`sui-adapter/src/static_programmable_transactions/mod.rs:38`](/home/vielite/hackenProof/sui/sui-execution/latest/sui-adapter/src/static_programmable_transactions/mod.rs:38)

**Purpose:**
- This function is the top-level handoff from the outer transaction engine into the Bela-Ciao static PTB path. Its role is to ensure raw programmable transaction input is not executed directly; instead, it is wrapped in a package cache, resolved against linkage, translated into an internal typed form, verified, and only then interpreted.
- The function exists to preserve a staged trust reduction pipeline. User-provided PTB data begins as untrusted transaction input and is progressively constrained into a linkage-aware and type-checked internal representation before any execution logic mutates runtime state.

**Inputs & Assumptions:**
- `protocol_config`, `metrics`, `vm`, `state_view`, `package_store`, `tx_context`, `gas_charger`, `withdrawal_compatibility_inputs`, `txn`, and `trace_builder_opt` are all implicit policy or state dependencies, not just raw parameters, at [`sui-adapter/src/static_programmable_transactions/mod.rs:38`](/home/vielite/hackenProof/sui/sui-execution/latest/sui-adapter/src/static_programmable_transactions/mod.rs:38) through [`sui-adapter/src/static_programmable_transactions/mod.rs:49`](/home/vielite/hackenProof/sui/sui-execution/latest/sui-adapter/src/static_programmable_transactions/mod.rs:49).
- Assumption 1: `gas_charger.gas_payment_amount()` is already consistent with outer-engine gas setup, because it is used as translation input before interpretation at L51 and L86.
- Assumption 2: `package_store` plus `state_view` contain enough package/object information to compute linkage for every PTB input type at L52-L58.
- Assumption 3: `vm.make_vm(...)` with the computed linkage context produces a Move VM view compatible with later type translation and execution at L59-L66.
- Assumption 4: translation must happen before typing because raw PTB arguments need environment-aware decoding into internal IR at L79-L89.
- Assumption 5: the typed transaction emitted by `typing::translate_and_verify` is sufficient for interpreter execution without re-decoding raw PTB structure at L91-L99.

**Outputs & Effects:**
- Produces either `Mode::ExecutionResults` plus timing data or an `ExecutionError` plus timing data as `ResultWithTimings`.
- Instantiates a cached package store at L52, which changes how package resolution is performed for the remainder of the execution pipeline.
- Instantiates a linkage-aware resolution VM at L59-L66 and an execution `Env` at L68-L75.
- Consumes gas-translation budget via `TranslationMeter::new` and downstream translation/typing calls at L76-L91.
- Delegates into the interpreter with a verified typed transaction at L93-L99.

**Block-by-Block Analysis:**

---

- Block: gas-payment snapshot and package-store wrapping at L51-L52.
- What: The function snapshots gas payment amount and wraps the backing package store in transaction-aware caching.
- Why here: Translation needs gas payment semantics and package resolution before any linkage or typing can be computed.
- Assumptions:
  - Gas payment amount is stable for this transaction phase.
  - Cached package access preserves correctness while reducing repeated lookup work.
- Depends on:
  - Outer `GasCharger` initialization in [`sui-adapter/src/execution_engine.rs:173`](/home/vielite/hackenProof/sui/sui-execution/latest/sui-adapter/src/execution_engine.rs:173).
- First Principles: the interpreter can only be correct if code and type identities are correct first; therefore package resolution must precede execution, because executing against the wrong package graph would make every later state transition meaningless.

---

- Block: linkage analysis at L53-L58.
- What: Computes input-type resolution linkage from the raw PTB and converts it into a linkage context.
- Why here: The VM view must be built with the exact package linkage implied by transaction inputs before translation and typing can interpret module/function/type identifiers.
- Assumptions:
  - Linkage depends on actual PTB inputs, not just global package state.
  - Failing linkage is fatal to execution and should stop the pipeline before any stateful work begins.
- Depends on:
  - `txn`, `package_store`, and `state_view`.
- 5 Whys:
  - Why compute linkage first? Because type names are not meaningful without package identity.
  - Why does package identity matter? Because the same logical name can resolve differently across versions/upgrades.
  - Why can that not be deferred? Because translation itself needs concrete type/module resolution.
  - Why is early failure valuable? Because it prevents partially built runtime context from existing under ambiguous code identity.
  - Why is that a system invariant? Because execution correctness depends on "typed IR references the exact code that will run."

---

- Block: resolution VM creation at L59-L66.
- What: Builds a VM instance bound to the previously computed linkage.
- Why here: The environment and translator both need a VM view already specialized to the PTB's linkage context.
- Assumptions:
  - `make_vm` is the point where code identity becomes operational, not just analytical.
  - Any failure here is classified as `InvalidLinkage`, meaning incorrect VM construction is treated as a linkage-layer failure.
- Depends on:
  - Successful linkage analysis.
- 5 Hows:
  - How does the function prevent ambiguous execution? By converting linkage context into a concrete resolution VM.
  - How does it preserve failure attribution? By mapping VM-construction failure into `ExecutionErrorKind::InvalidLinkage`.
  - How does that help later reasoning? It keeps package/linkage faults distinct from runtime execution faults.

---

- Block: environment and translation meter construction at L68-L77.
- What: Creates the execution environment and translation metering state.
- Why here: All later translation and typing steps need a single shared view over protocol config, VM, state, and package resolution.
- Assumptions:
  - Translation costs are charged before runtime execution starts.
  - `Env` is the stable anchor that bridges translation, typing, and interpreter phases.
- Depends on:
  - `resolution_vm`, `linkage_analysis`, and cached package store.
- 5 Whys:
  - Why centralize environment creation? Because each downstream phase must reason over the same package and state view.
  - Why meter translation separately? Because resource accounting starts before command execution.
  - Why does that matter? Because otherwise raw PTB complexity could escape gas/resource attribution.

---

- Block: loading translation at L79-L89.
- What: Borrows `TxContext` and translates the raw programmable transaction into an internal transaction representation.
- Why here: Translation must happen before typing because the type system checks the internal AST, not the external wire format.
- Assumptions:
  - `tx_context` contributes to translation semantics, not just execution semantics.
  - `withdrawal_compatibility_inputs` changes how certain inputs are materialized.
- Depends on:
  - `Env`, translation meter, gas payment amount, and transaction context.
- 5 Hows:
  - How is raw input constrained? By passing it through `loading::translate::transaction`.
  - How is transaction-scoped state injected? Through `tx_context_ref`, gas-payment amount, and withdrawal compatibility inputs.
  - How is failure handled? Immediate error return before type verification or execution.

---

- Block: typing/verification and interpreter handoff at L91-L99.
- What: Converts the translated transaction into a verified typed transaction, then executes it.
- Why here: Execution is intentionally the last stage, after all structure and type obligations have been satisfied.
- Assumptions:
  - The interpreter trusts the typed AST more than it trusts the original PTB.
  - No additional raw-input validation is expected inside the interpreter for properties already enforced by translation/typing.
- Depends on:
  - Successful translation output and shared environment.
- First Principles: execution should operate on the smallest possible trusted surface. The typed AST is smaller and more constrained than the raw PTB, so the architecture reduces trust before granting mutation capability.

**Cross-Function Dependencies:**
- Called from the outer engine when the transaction kind routes into static PTB execution; the earlier note identifies this path under [`sui-adapter/src/execution_engine.rs:119`](/home/vielite/hackenProof/sui/sui-execution/latest/sui-adapter/src/execution_engine.rs:119).
- Depends on `loading::translate::transaction` at L81-L88 for raw-to-internal transformation.
- Depends on `typing::translate_and_verify` at L91-L92 for typed safety before runtime execution.
- Depends on `execution::interpreter::execute` at L93-L99, which consumes the typed transaction and mutates state.
- Shared state coupling:
  - `gas_charger` spans translation and execution, so invariants about gas accounting cross phase boundaries.
  - `tx_context` is shared across translation and execution, so ID derivation and transaction identity must remain coherent across both.
- Invariants:
  - Invariant 1: execution only starts after successful linkage, translation, and typing.
  - Invariant 2: all later execution uses the same linkage-aware VM environment built here.
  - Invariant 3: translation and typing failures must leave no partially initialized interpreter context.

## Function: `Context::new`
Source: [`sui-adapter/src/static_programmable_transactions/execution/context.rs:301`](/home/vielite/hackenProof/sui/sui-execution/latest/sui-adapter/src/static_programmable_transactions/execution/context.rs:301)

**Purpose:**
- This function materializes the typed PTB into concrete runtime state. It is the boundary where abstract typed inputs become interpreter locals, gas objects, and native-extension object-runtime state.
- The function exists because the interpreter cannot execute against symbolic input descriptors. It needs actual `Locals`, object metadata, pure input byte tables, receiving slots, and a synchronized `ObjectRuntime` that knows the initial object universe for this transaction.

**Inputs & Assumptions:**
- Inputs include `env`, `metrics`, shared `tx_context`, mutable `gas_charger`, optional `payment_location`, pure bytes and metadata, object inputs, withdrawal metadata, and receiving metadata at L303-L312.
- Assumption 1: typed object inputs are already structurally valid enough for `load_object_arg` to convert each into runtime values at L320-L324.
- Assumption 2: withdrawal inputs are semantically distinct from object inputs and need a separate loading path via `load_withdrawal_arg` at L326-L330.
- Assumption 3: pure and receiving inputs begin invalid on purpose at L332-L333 and are only populated lazily/through dedicated resolution logic later.
- Assumption 4: gas payment can be represented either as a concrete coin object or as an address-balance-backed synthetic coin at L335-L380.
- Assumption 5: `adapter::new_native_extensions` must receive a complete `input_object_map` so `ObjectRuntime` starts with a faithful view of loaded inputs at L384-L391.

**Outputs & Effects:**
- Returns a fully initialized `Context` or an `ExecutionError`.
- Loads all object inputs into `Locals` and records parallel metadata at L317-L325.
- Loads withdrawal inputs into a separate local frame at L326-L331.
- Potentially creates a synthetic gas coin with a fresh object ID at L345-L363.
- Debits the runtime-visible gas coin by the full gas budget before command execution at L374-L380.
- Creates native extensions and potentially registers a newly created gas coin in `ObjectRuntime` at L384-L405.
- Creates a `TxContext` local value at L407-L410 and stores all local groups into `locations` at L418-L430.

**Block-by-Block Analysis:**

---

- Block: object-input loading at L317-L325.
- What: Iterates through all typed object inputs, loads each object, records metadata, and turns the resulting values into a local frame.
- Why here: Object inputs are the primary stateful resources the interpreter can mutate or inspect, so they must exist before any command execution or gas/native-extension setup.
- Assumptions:
  - `load_object_arg` returns a tuple whose metadata and value correspond to the same logical object.
  - `input_object_map` needs to be populated during loading, not reconstructed later.
- Depends on:
  - Typed object descriptors produced earlier by translation/typing.
- 5 Whys:
  - Why track both metadata and values? Because execution needs values while post-execution reconciliation needs metadata.
  - Why is this separated from later finish logic? Because the runtime must preserve origin information through the whole command sequence.
  - Why is the map built incrementally? Because native extensions later need the complete initial object set.

---

- Block: withdrawal/pure/receiving locals initialization at L326-L333.
- What: Loads withdrawal inputs, then creates invalid local slots for pure and receiving inputs.
- Why here: The context must allocate every input class up front so later location resolution can index into stable local arrays.
- Assumptions:
  - Pure/receiving data are not immediately materialized as move values at construction time.
  - Slot count must match metadata count exactly.
- Depends on:
  - Input metadata lengths and withdrawal loading helpers.
- 5 Hows:
  - How does the function distinguish input classes? By maintaining separate local collections per input type.
  - How does it preserve positional identity? By sizing locals directly from metadata vector lengths.
  - How does it avoid premature deserialization? By leaving pure/receiving locals invalid initially.

---

- Block: gas handling dispatch at L334-L383.
- What: Decides whether gas is absent, synthetic-from-address-balance, or loaded from a real coin object, then subtracts the gas budget from the runtime-visible balance.
- Why here: Gas needs to be represented as part of runtime locals before commands execute, because commands may read, borrow, split, merge, or transfer the gas coin depending on protocol rules.
- Assumptions:
  - `gasless_transaction_drop_safety()` gates whether address-balance-backed gas should be materialized as a runtime coin at L337-L340.
  - When synthetic gas is created, the transaction can safely allocate a fresh ID from `TxContext` at L351-L352.
  - The gas budget has already been checked against available balance before `coin_ref_subtract_balance` is called at L377-L379.
- Depends on:
  - `payment_location`, `env.protocol_config`, and `gas_charger`.
- First Principles: runtime execution needs a single coherent model for "the gas object visible to Move code." Whether the underlying economic source is a real coin or an address balance, the interpreter still needs one normalized representation, otherwise command semantics would branch on funding source everywhere.
- 5 Whys:
  - Why convert address balance into a coin-like value? Because the rest of the runtime expects gas in object/value form.
  - Why subtract the full gas budget immediately? Because execution should start from spendable remainder, not full face value.
  - Why do this before native extensions are built? Because the extensions should observe the same initial object/balance state the interpreter will use.

---

- Block: native-extension construction at L384-L391.
- What: Builds the native extension set, including object-runtime state, from child resolver access, input object map, metering flag, protocol config, metrics, and transaction context.
- Why here: Extensions must be constructed after all initial inputs are known but before the interpreter uses them.
- Assumptions:
  - `input_object_map` contains every relevant loaded object at this point.
  - Metering mode affects native-extension behavior.
- Depends on:
  - Previously loaded object map and current gas-charger state.
- 5 Hows:
  - How does object runtime learn the transaction’s initial object universe? Through `input_object_map`.
  - How does native behavior stay transaction-specific? By passing `tx_context` and protocol config into extension creation.
  - How does the function tie execution to observability? By threading metrics into extension creation.

---

- Block: synthetic gas coin registration at L392-L405.
- What: If a new gas coin ID was minted, registers it with `ObjectRuntime`.
- Why here: The object runtime must know about all created IDs before commands run, otherwise later object-tracking invariants would start from an incomplete set.
- Assumptions:
  - New synthetic gas IDs must be treated like other newly created runtime objects.
  - Borrowing the native extension mutably should always succeed here unless there is an internal invariant break.
- Depends on:
  - Successful native-extension construction and optional synthetic gas creation.
- 5 Whys:
  - Why register the new gas ID explicitly? Because it did not come from input-object loading.
  - Why is that important? Because object-runtime accounting relies on exact object identity tracking.
  - Why convert failure into VM error via `env.convert_vm_error`? Because extension/object-runtime failures must surface in the same execution error channel as other VM-local failures.

---

- Block: tx-context local creation and final struct assembly at L407-L430.
- What: Asserts stack-height sanity, creates a Move-visible `TxContext` local, and assembles the final `Context`.
- Why here: The runtime should only become visible to the interpreter after all inputs, gas, and extensions are coherent.
- Assumptions:
  - Move stack height must be zero before execution begins.
  - `Value::new_tx_context(tx_context.borrow().digest())` is sufficient to seed interpreter-visible transaction context.
- Depends on:
  - All prior loading and extension work.
- 5 Hows:
  - How does the function ensure a clean execution start? By asserting zero stack height before returning.
  - How does it preserve all input classes for later resolution? By storing each in `locations`.
  - How does it create a single runtime anchor? By returning one `Context` object carrying env, charger, extensions, locals, and metrics together.

**Cross-Function Dependencies:**
- Called directly by [`sui-adapter/src/static_programmable_transactions/execution/interpreter.rs:84`](/home/vielite/hackenProof/sui/sui-execution/latest/sui-adapter/src/static_programmable_transactions/execution/interpreter.rs:84).
- Depends on helper loaders `load_object_arg`, `load_withdrawal_arg`, and `load_object_arg_impl` referenced at L321, L328, and L365.
- Depends on `adapter::new_native_extensions` at L384-L391 to bridge execution state into native object-runtime support.
- Shares `tx_context` and `gas_charger` with the caller, so transaction identity and gas invariants span the caller/callee boundary.
- Invariant couplings:
  - Invariant 1: every object input loaded into locals has matching metadata preserved in `input_object_metadata`.
  - Invariant 2: if a synthetic gas coin is created, its ID is also registered in `ObjectRuntime`.
  - Invariant 3: interpreter-visible gas balance begins after budget subtraction, not before.
  - Invariant 4: pure and receiving inputs keep stable slot identity even before being materialized.

## Function: `interpreter::execute_inner`
Source: [`sui-adapter/src/static_programmable_transactions/execution/interpreter.rs:61`](/home/vielite/hackenProof/sui/sui-execution/latest/sui-adapter/src/static_programmable_transactions/execution/interpreter.rs:61)

**Purpose:**
- This function is the core typed PTB execution loop. It receives the already verified typed transaction, builds the concrete runtime `Context`, executes each typed command in order, and then persists runtime results back into the outer `ExecutionState`.
- Its role in the system is sequencing and continuity. It preserves the invariant that the entire typed PTB executes against one coherent context, while still recording per-command timing and enough runtime-object information for replay, storage-rebate calculation, and post-execution checks.

**Inputs & Assumptions:**
- Inputs are mutable `timings`, mutable `env`, shared `tx_context`, mutable `gas_charger`, typed `ast`, and optional trace builder at L61-L68.
- Assumption 1: the `ast` has already passed translation and verification, so the interpreter can destructure it directly at L74-L83.
- Assumption 2: `Context::new` produces all runtime locals and native extensions needed for every command at L84-L95.
- Assumption 3: command order is semantically meaningful and preserved by the `for sp!(idx, c) in commands` loop at L100.
- Assumption 4: even on command failure, loaded runtime objects must still be saved for replay/debug state at L105-L114.
- Assumption 5: successful completion requires both command-loop success and successful `context.finish::<Mode>()` plus downstream recording into `env.state_view` at L130-L139.

**Outputs & Effects:**
- Returns `Mode::ExecutionResults` or an `ExecutionError`.
- Appends per-command success or abort timings to `timings` at L101-L116.
- Creates the runtime `Context` at L84-L95.
- Emits PTB tracing summary before command execution at L97.
- On failure, saves loaded runtime objects and returns an error annotated with the failing command index at L105-L114.
- On success, extracts loaded runtime objects, wrapped object containers, and generated object IDs at L123-L128.
- Finalizes the context and records execution results into the state view at L130-L139.

**Block-by-Block Analysis:**

---

- Block: initial sanity and AST destructuring at L73-L83.
- What: Asserts clean Move stack state and decomposes the typed transaction into gas/input/command components.
- Why here: Execution should begin only from a fully normalized typed transaction and a clean VM gas stack state.
- Assumptions:
  - Stack height zero is a required precondition for running the PTB.
  - The typed transaction includes all data needed to build runtime context without touching raw PTB input again.
- Depends on:
  - Prior translation/typing done in `static_programmable_transactions::execute`.
- First Principles: once execution starts, it should operate on normalized internal data only. Re-reading raw input during execution would re-expand the trusted surface and undermine the staged architecture.

---

- Block: context creation and trace summary at L84-L97.
- What: Builds runtime context from the typed transaction parts, then records a PTB-level trace summary.
- Why here: No command can execute until runtime locals and extensions exist; the summary trace needs the initialized context plus the pending command list.
- Assumptions:
  - `Context::new` is the sole constructor for valid interpreter runtime state.
  - Tracing the command list before execution is useful and side-effect-safe.
- Depends on:
  - `Context::new` and `trace_utils::trace_ptb_summary`.
- 5 Whys:
  - Why instantiate context once up front? Because commands share mutable state and local slots across the full transaction.
  - Why trace before the loop? Because this captures the intended execution plan before any mutation or failure alters state.
  - Why is that important? Because debugging/replay need both pre-state shape and post-failure runtime-object data.

---

- Block: command execution loop at L99-L116.
- What: Iterates over typed commands, executes each one, records timing, and aborts on first error.
- Why here: PTBs are ordered programs; command outputs and mutations feed later commands, so the loop enforces sequential semantics.
- Assumptions:
  - `execute_command` has complete authority over each command’s semantics.
  - Timings should be recorded regardless of success or failure.
- Depends on:
  - Mutable `context`, mutable `mode_results`, and per-command typed command values.
- 5 Hows:
  - How is sequential semantics preserved? By iterating commands in source order and mutating one shared `context`.
  - How is failure localized? By attaching `idx` to the returned error at L114.
  - How is observability preserved? By recording `ExecutionTiming::Success` or `Abort` for each attempted command.

---

- Block: error-path runtime-object preservation at L102-L114.
- What: On command failure, extracts loaded runtime objects, drops the context, saves those objects into the state view, records abort timing, and returns the indexed error.
- Why here: The engine still needs runtime-object loading information even when the transaction aborts.
- Assumptions:
  - Loaded child objects matter for replay/debug/storage accounting even on failure.
  - Wrapped objects should not be saved on error because they should not be modified, per the code comment at L107.
- Depends on:
  - `object_runtime!(context)?` and `env.state_view`.
- 5 Whys:
  - Why preserve loaded runtime objects on failure? Because failure does not erase the fact that runtime resolution happened.
  - Why drop `context` before saving? Because borrowing/lifetime constraints require releasing the runtime borrow before using `env.state_view`.
  - Why annotate the error with command index? Because PTBs are sequences; debugging requires exact failure position.

---

- Block: success-path runtime extraction and finish at L118-L140.
- What: Extracts runtime-object bookkeeping, finalizes the context, persists execution results and metadata into the state view, and returns mode results.
- Why here: Final state should only be committed once every command has succeeded.
- Assumptions:
  - `context.finish::<Mode>()` applies buffered runtime changes and may still fail after all commands succeeded.
  - Loaded runtime objects, wrapped containers, and generated IDs are needed by downstream accounting or invariant checks.
- Depends on:
  - Successful completion of the entire command loop.
- 5 Hows:
  - How does the interpreter preserve post-execution observability? By saving loaded objects, wrapped object containers, and generated IDs before returning.
  - How does it keep state changes atomic at this layer? By calling `context.finish::<Mode>()` only after all commands succeed.
  - How does it bridge runtime to outer engine state? By recording execution results into `env.state_view`.

**Cross-Function Dependencies:**
- Called from [`sui-adapter/src/static_programmable_transactions/execution/interpreter.rs:42`](/home/vielite/hackenProof/sui/sui-execution/latest/sui-adapter/src/static_programmable_transactions/execution/interpreter.rs:42) through the wrapper `execute`.
- Directly depends on `Context::new` at L84-L95.
- Directly depends on `execute_command` at L102-L103 for actual command semantics.
- Directly depends on `context.finish::<Mode>()` at L131 to materialize buffered changes.
- Shared state coupling:
  - `env.state_view` is the bridge from interpreter-local execution back to the outer transaction engine.
  - `gas_charger` spans this function, `Context::new`, and per-command execution; gas invariants must hold across all three layers.
- Invariants:
  - Invariant 1: all commands run against one shared mutable context.
  - Invariant 2: failure returns the first failing command index and still preserves loaded runtime-object information.
  - Invariant 3: final execution results are not recorded into `state_view` until after `context.finish::<Mode>()` succeeds.

## Phase 3: Current Global Understanding

### Reconstructed state relationships

- Outer-engine setup establishes transaction-wide identity and resource constraints before the static PTB path begins:
  - `TemporaryStore` is the journal anchor.
  - `GasCharger` is the gas/accounting anchor.
  - `TxContext` is the transaction-identity and fresh-ID anchor.
- Static PTB execution narrows trust in stages:
  - raw PTB
  - linkage-aware translation
  - typed verification
  - runtime context materialization
  - sequential command execution
  - final state recording
- `Context::new` is the critical semantic conversion point where input classes are normalized into runtime slots and object-runtime support.

### Reconstructed workflow

1. `execute_transaction_to_effects` creates the transaction-local execution shell in [`sui-adapter/src/execution_engine.rs:119`](/home/vielite/hackenProof/sui/sui-execution/latest/sui-adapter/src/execution_engine.rs:119) through [`sui-adapter/src/execution_engine.rs:209`](/home/vielite/hackenProof/sui/sui-execution/latest/sui-adapter/src/execution_engine.rs:209).
2. Static PTB `execute` computes linkage, constructs a resolution VM, translates raw PTB, verifies typed IR, and calls the interpreter in [`sui-adapter/src/static_programmable_transactions/mod.rs:51`](/home/vielite/hackenProof/sui/sui-execution/latest/sui-adapter/src/static_programmable_transactions/mod.rs:51) through [`sui-adapter/src/static_programmable_transactions/mod.rs:99`](/home/vielite/hackenProof/sui/sui-execution/latest/sui-adapter/src/static_programmable_transactions/mod.rs:99).
3. `execute_inner` constructs `Context`, executes typed commands in order, and records finalized runtime results in [`sui-adapter/src/static_programmable_transactions/execution/interpreter.rs:84`](/home/vielite/hackenProof/sui/sui-execution/latest/sui-adapter/src/static_programmable_transactions/execution/interpreter.rs:84) through [`sui-adapter/src/static_programmable_transactions/execution/interpreter.rs:140`](/home/vielite/hackenProof/sui/sui-execution/latest/sui-adapter/src/static_programmable_transactions/execution/interpreter.rs:140).

### Trust boundaries

- Untrusted boundary 1: raw PTB and its referenced inputs at static PTB entry.
- Boundary reduction 1: linkage analysis and resolution VM creation constrain package/type identity.
- Boundary reduction 2: translation and typing reduce the raw PTB into a verified typed internal AST.
- Boundary reduction 3: `Context::new` turns typed descriptions into concrete runtime state while preserving origin metadata and object-runtime tracking.
- Boundary reduction 4: `context.finish::<Mode>()` bridges interpreter-local state back to outer execution state.

### Fragility clusters for deeper future review

- Gas normalization logic in `Context::new`, especially address-balance-backed synthetic gas behavior.
- Object-runtime/native-extension seeding, because bookkeeping correctness here affects every later object-level invariant.
- Error-path state preservation in the interpreter, because replay/accounting/debug correctness depends on what survives aborts.
- The raw-PTB-to-typed-IR transition, because the interpreter assumes the typed AST is trustworthy enough not to re-validate earlier properties.
