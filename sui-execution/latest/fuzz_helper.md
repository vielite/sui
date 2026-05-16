# Phase 1 Fuzz Helper for `sui-execution/latest`

This note turns the Phase 1 context in `audit_context_building.md` into a concrete fuzzing helper for low-resource research.

Scope:
- `sui-adapter/src/static_programmable_transactions/mod.rs::execute`
- `sui-adapter/src/static_programmable_transactions/execution/context.rs::Context::new`
- `sui-adapter/src/static_programmable_transactions/execution/interpreter.rs::execute_inner`

The goal here is not to claim bugs. The goal is to extract the highest-value invariants and convert them into practical fuzz targets that are still usable on weak hardware.

## Key Repo Observations

- There is no dedicated fuzz harness checked into `sui-execution/latest` itself.
- The cheapest existing in-tree fuzz references are outside this subtree:
  - `crates/transaction-fuzzer` for deterministic/property-style local fuzzing
  - `external-crates/move/crates/bytecode-verifier-libfuzzer` for minimal `cargo-fuzz` style harnesses
- For a low-end machine, the right approach is to fuzz the Phase 1 transition boundaries directly, not the full outer execution engine.

## Recommended Target Order

1. `Context::new`
2. `execute_inner`
3. `mod.rs::execute`

Rationale:
- `Context::new` is the narrowest point where typed PTB inputs become runtime locals, gas state, and native extensions.
- `execute_inner` adds command sequencing and post-execution persistence, but still stays inside the static PTB executor.
- `mod.rs::execute` is the broadest of the Phase 1 functions because it includes linkage setup, translation, typing, and interpreter dispatch.

## Low-Resource Strategy

Use this order of operations:

1. Read-driven invariant harvesting first.
2. Property-based testing with small structured inputs.
3. Tiny deterministic repro loops before coverage-guided fuzzing.
4. Only then use `cargo-fuzz`, and only on a very small harness.

Practical constraints for weak hardware:

- Build one crate at a time.
- Force low parallelism: `CARGO_BUILD_JOBS=1`
- Drop debug info while iterating: `RUSTFLAGS='-C debuginfo=0'`
- Keep corpora tiny.
- Cap command counts, input counts, and object counts aggressively.
- Prefer CPU-bound verifier/translation/context seams over stateful full-engine execution.

Useful commands:

```bash
CARGO_BUILD_JOBS=1 RUSTFLAGS='-C debuginfo=0' cargo test -p sui-adapter-latest
CARGO_BUILD_JOBS=1 RUSTFLAGS='-C debuginfo=0' cargo test -p sui-verifier-latest
```

If a tiny libFuzzer harness is added later:

```bash
CARGO_BUILD_JOBS=1 RUSTFLAGS='-C debuginfo=0' cargo fuzz run <target> -- -max_total_time=300 -rss_limit_mb=2048
```

## Crash-Oriented Fuzzing

Crash fuzzing should be explicit here. For this Phase 1 slice, the main crash oracles are:

- process panic
- `assert_invariant!` failure
- `invariant_violation!` / `make_invariant_violation!` paths
- unexpected `debug_assert!` failure in debug builds
- borrow-state crashes around native extensions and locals

Treat these as first-class outcomes, not just side effects of semantic fuzzing.

Recommended crash oracles:

1. Debug build oracle
   - Run a small deterministic/property corpus under a debug build.
   - Goal: surface `debug_assert!` failures and panic-only invariants early.

2. Release build oracle
   - Re-run minimized inputs in release-like settings.
   - Goal: distinguish debug-only crashes from real `ExecutionInvariantViolation` style failures.

3. Panic capture oracle
   - Wrap direct harness entrypoints with `std::panic::catch_unwind` when practical.
   - Goal: record panic inputs cleanly instead of losing the corpus item.

4. Error-kind oracle
   - Distinguish expected user/input errors from invariant failures.
   - Goal: avoid wasting time on ordinary validation failures.

Crash scenarios worth prioritizing:

- missing or suppressed gas-local state when typed locations still mention `GasCoin`
- duplicate or aliased object identities reaching runtime materialization
- invalid local-slot assumptions for pure or receiving inputs
- native-extension borrow conflicts
- object-runtime bookkeeping drift between locals and `ObjectRuntime`
- coin balance underflow or overflow along gas setup and early command execution
- malformed object/type/layout combinations that survive translation but fail during runtime loading

## Function 1: `static_programmable_transactions::execute`

Location:
- `sui-adapter/src/static_programmable_transactions/mod.rs:38-100`

Purpose:
- This is the static PTB pipeline entry for programmable transactions.
- It snapshots gas-payment state, builds package/linkage context, translates the raw PTB, type-checks it, and only then calls the interpreter.

Critical invariants:

1. The gas-payment snapshot taken at line 51 must remain the gas-payment input used for translation.
   - Evidence: `let gas_payment = gas_charger.gas_payment_amount();` at line 51 is later passed to `loading::translate::transaction` at line 86.
   - Fuzz implication: mutation around translator inputs should not create divergence between `gas_charger` state and the gas-payment amount consumed by translation.

2. The linkage context used to build `resolution_vm` must be derived from the same raw `txn` being translated.
   - Evidence: lines 55-58 compute `ptb_type_linkage` from `txn`, and lines 59-66 use it to build `resolution_vm`.
   - Fuzz implication: malformed PTBs and malformed package/linkage states should fail before interpretation, not produce a VM built from mismatched linkage.

3. Translation and typing must execute against the same environment stack.
   - Evidence: `Env::new` at lines 68-75 captures `protocol_config`, `state_view`, `package_store`, `linkage_analysis`, and `resolution_vm`; the same `env` is then used for loading translation at lines 81-88 and typing verification at lines 91-92.
   - Fuzz implication: any harness that stubs one stage but not the other risks missing cross-stage mismatches that this function explicitly prevents.

4. The translation meter is shared across loading and typing, so metering is monotonic across the whole PTB preparation path.
   - Evidence: `translation_meter` is created once at lines 76-77, then passed to both translation and typing at lines 82 and 91.
   - Fuzz implication: metering-related bugs should be hunted with multi-stage inputs, not by fuzzing translation and typing as isolated worlds.

5. `tx_context` must be stable during translation.
   - Evidence: a borrowed `tx_context_ref` is passed at lines 79-89.
   - Fuzz implication: any harness that mutates `TxContext` mid-translation is modeling a state transition this function does not permit.

6. No interpreter execution occurs unless linkage, VM construction, loading translation, and typing verification all succeed.
   - Evidence: each stage returns `?` before the final call at lines 93-99.
   - Fuzz implication: failures found before interpreter dispatch are still valid high-signal results; they are on the intended trust boundary.

Best fuzz angles:

- Raw PTB shape vs linkage mismatch.
- Withdrawal compatibility flags vs translator expectations.
- Metering edge cases across translation plus typing in one run.
- Inputs that force `InvalidLinkage` or verifier-style structural failures before interpreter entry.

Cheap harness idea:

- Property-based generator for very small `ProgrammableTransaction` values.
- Constrain to:
  - 0-3 commands
  - 0-4 inputs
  - a tiny package-store fixture
- Success condition:
  - no panic
  - deterministic error classification for malformed linkage/type states

Crash scenarios:

- linkage resolution producing a VM state that later stages cannot safely consume
- translation/type outputs that are structurally accepted but trigger invariant failures during interpreter handoff
- malformed package-store fixtures causing unexpected panics instead of typed linkage errors

Crash oracle additions:

- any panic during linkage analysis, VM construction, translation, or typing is a bug candidate
- any invariant-violation-style error returned before interpreter dispatch is high-signal and should be minimized
- expected user-facing linkage/type failures should be bucketed separately from crash-like failures

## Function 2: `Context::new`

Location:
- `sui-adapter/src/static_programmable_transactions/execution/context.rs:301-433`

Purpose:
- This is the materialization boundary from typed PTB data into runtime locals, gas state, `TxContext` value, and native extensions.
- If the runtime model is wrong here, later interpreter logic can be locally correct while still operating over inconsistent state.

Critical invariants:

1. Every loaded object input must produce matching metadata and runtime value entries in lockstep.
   - Evidence: lines 317-324 push `(i, m)` into `input_object_metadata` and `Some(v)` into `object_values` from the same `load_object_arg` result.
   - Fuzz implication: try to break positional coupling, duplicate IDs, or metadata/value mismatches.

2. The number of runtime object locals must exactly match the number of typed object inputs.
   - Evidence: `Locals::new(object_values)` at line 325 is built directly from one value per loop iteration.
   - Fuzz implication: bounds, duplication, and aliasing bugs around object indexing should show up here first.

3. Withdrawal locals must preserve one-for-one alignment with withdrawal metadata.
   - Evidence: lines 326-331 build one local per `input_withdrawal_metadata` item.
   - Fuzz implication: any mismatch between withdrawal metadata shape and loaded withdrawal values is a prime target.

4. Pure and receiving locals are intentionally invalid at construction time, but their slot counts must still match metadata lengths.
   - Evidence: `Locals::new_invalid(pure_input_metadata.len())` at line 332 and `Locals::new_invalid(receiving_input_metadata.len())` at line 333.
   - Fuzz implication: lazy-materialization bugs should be tested with incorrect metadata lengths, repeated reads, and partially consumed inputs.

5. If gas comes from `AddressBalance` and `gasless_transaction_drop_safety()` is disabled, no runtime gas local is created.
   - Evidence: lines 335-340 return `None`.
   - Fuzz implication: this is an explicit semantic branch worth differential fuzzing against the enabled-safety branch.

6. If gas comes from `AddressBalance` and runtime gas is created, the amount must cover the gas budget before materialization proceeds.
   - Evidence: invariant assertion at lines 346-350.
   - Fuzz implication: insufficient-balance states should fail cleanly and never reach partially initialized gas state.

7. Synthetic address-balance gas must use a fresh object ID and be marked as a newly created mutable address-owned object.
   - Evidence: lines 351-360 create `fresh_id()`, set `newly_created: true`, `mutability: Mutable`, and `owner: AddressOwner`.
   - Fuzz implication: ID-generation, ownership labeling, and created-vs-loaded bookkeeping are core attack surfaces.

8. Real gas-coin payment must be loaded through the same object-loading machinery and inserted into the input object map.
   - Evidence: lines 365-372 call `load_object_arg_impl(..., &mut input_object_map, ...)`.
   - Fuzz implication: gas/object aliasing and duplicate-ID paths matter because gas joins the same runtime model.

9. The runtime-visible gas value is reduced by the full gas budget before any command executes.
   - Evidence: lines 374-380 build a gas local and call `coin_ref_subtract_balance(max_gas_in_balance)`.
   - Fuzz implication: coin-splitting and transfer logic later should be fuzzed assuming the visible gas balance is already net-of-budget.

10. Native extensions must be constructed from a complete initial `input_object_map`.
   - Evidence: lines 384-391 pass `input_object_map` into `adapter::new_native_extensions`.
   - Fuzz implication: any object omitted here produces a runtime blind spot for later object-runtime behavior.

11. If a synthetic gas coin was created, that ID must also be registered with `ObjectRuntime`.
   - Evidence: lines 392-405 call `object_runtime.new_id(new_gas_coin_id)`.
   - Fuzz implication: created-object tracking should be fuzzed for divergence between locals and object runtime.

12. Move VM stack height must still be zero after context initialization.
   - Evidence: `debug_assert_eq!(...stack_height_current(), 0)` at line 407.
   - Fuzz implication: helper calls inside initialization must not leak stack effects across the construction boundary.

13. The transaction context local must always be present and valid in slot 0 of `tx_context_value`.
   - Evidence: lines 408-410 create exactly one tx-context value, then lines 418-430 store it into `locations`.
   - Fuzz implication: later finish logic depends on this local never being missing.

Best fuzz angles:

- Object input metadata/value alignment.
- Gas branch differentials:
  - `None`
  - real gas coin
  - synthetic address-balance gas
  - address-balance gas suppressed by protocol config
- Duplicate object IDs across object inputs and gas input.
- Large or malformed pure/receiving metadata with zero actual materialization.
- New-object registration mismatches between locals and `ObjectRuntime`.

Cheap harness idea:

- Construct tiny typed input sets directly for `Context::new`.
- Cap sizes to:
  - objects: 0-4
  - withdrawals: 0-2
  - pure inputs: 0-4
  - receiving inputs: 0-2
- Check only:
  - no panic
  - slot-count consistency
  - gas-branch postconditions
  - created-object registration consistency

This is the best first fuzz target on a weak machine.

Crash scenarios:

- object loader returns metadata/value combinations that later helpers cannot serialize or borrow safely
- `PaymentLocation::AddressBalance` plus protocol/config combinations leaving runtime gas assumptions inconsistent
- synthetic gas coin is created locally but not mirrored correctly into `ObjectRuntime`
- `Locals::new_invalid(...)` slot setup later collides with unexpected eager access patterns
- native extension setup or `try_borrow_mut()` assumptions fail under unusual initialization shapes
- deserialization/layout resolution for object inputs panics or produces invariant-violation paths

Crash oracle additions:

- panic in `load_object_arg`, `load_object_arg_impl`, `load_withdrawal_arg`, or `adapter::new_native_extensions`
- `ExecutionInvariantViolation` from:
  - gas-presence assumptions
  - object-runtime registration assumptions
  - tx-context local presence assumptions
- any debug assertion on stack height or local validity

Review note after manual triage:

- I did not confirm a High or Medium severity bug in `Context::new` itself.
- The most plausible candidate was the branch where address-balance gas may suppress the runtime gas local:
  - `sui-adapter/src/static_programmable_transactions/execution/context.rs:335-340`
- I do not currently treat that as a live H/M issue for `latest` because:
  - older address-balance configurations reject `Argument::GasCoin` upstream during transaction validity checks in `crates/sui-types/src/transaction.rs:2959-2973`
  - protocol versioning enables `gasless_transaction_drop_safety` before the `latest` execution version is selected in `crates/sui-protocol-config/src/lib.rs:4688-4707`
  - the supported modern address-balance gas path is exercised by transactional tests under `crates/sui-adapter-transactional-tests/tests/address_balances/`

What remains worth fuzzing here is not an already-confirmed exploit path, but regression risk:

- config drift between `address_balance_gas_reject_gas_coin_arg` and `gasless_transaction_drop_safety`
- mismatches between synthetic gas-coin local state and `ObjectRuntime` bookkeeping
- future changes that make the suppressed-gas-local branch reachable with `GasCoin`-using PTBs again

## Function 3: `execute_inner`

Location:
- `sui-adapter/src/static_programmable_transactions/execution/interpreter.rs:61-140`

Purpose:
- This is the typed PTB interpreter driver.
- It creates the execution context, runs each typed command, records per-command timings, snapshots runtime object metadata, and persists post-execution state into `state_view`.

Critical invariants:

1. The Move gas stack height must be zero before interpreter execution begins.
   - Evidence: line 73.
   - Fuzz implication: any path entering `execute_inner` with residual stack state is already violating a core execution assumption.

2. The AST components passed into `Context::new` must be forwarded unchanged from the typed transaction.
   - Evidence: lines 74-95 destructure `T::Transaction` and pass the parts directly into `Context::new`.
   - Fuzz implication: if a harness rewrites fields between typing and execution, it is no longer modeling the real trust boundary.

3. Command execution is strictly sequential and aborts on the first command error.
   - Evidence: lines 100-115 loop over commands and return immediately on error.
   - Fuzz implication: short command sequences are enough to explore many meaningful states; you do not need long corpora on weak hardware.

4. Every attempted command records exactly one timing outcome.
   - Evidence: lines 101-116 push either `ExecutionTiming::Abort` or `ExecutionTiming::Success`.
   - Fuzz implication: timing vector length is a cheap invariant oracle.

5. On command failure, loaded runtime objects must still be saved for replay/debug, but wrapped-object state is intentionally not saved.
   - Evidence: lines 105-114 save only `loaded_runtime_objects` after `drop(context)`.
   - Fuzz implication: failure paths need dedicated testing; they preserve only a subset of runtime artifacts by design.

6. Command errors must be annotated with the failing command index.
   - Evidence: line 114 calls `err.with_command_index(idx as usize)`.
   - Fuzz implication: index propagation is a simple, strong correctness oracle for command-sequence fuzzing.

7. On the success path, runtime object snapshots must be taken before `context.finish()`.
   - Evidence: lines 123-128 collect `loaded_runtime_objects`, `wrapped_object_containers`, and `generated_object_ids`, then line 131 calls `context.finish::<Mode>()`.
   - Fuzz implication: post-finish access to runtime extensions is not the intended model; snapshot timing matters.

8. Persistence into `state_view` occurs in a fixed order after `finish`.
   - Evidence:
     - save loaded objects at lines 133-134
     - save wrapped containers at lines 135-136
     - record execution results at line 137
     - record generated IDs at lines 138-139
   - Fuzz implication: state-capture ordering bugs are worth checking with injected post-finish failures.

9. `mode_results` is returned separately from `finished` execution results.
   - Evidence: line 140 returns `mode_results`, while line 137 persists `finished?` into `state_view`.
   - Fuzz implication: a harness should distinguish interpreter-returned mode results from persisted journal/effects state.

Best fuzz angles:

- Tiny typed command sequences:
  - 0-3 commands
  - especially `SplitCoins`, `MergeCoins`, `TransferObjects`, and a minimal `MoveCall`
- Failure-path persistence behavior.
- Command-index propagation on failure.
- Differential behavior between empty command lists and one-command abort sequences.
- Success-vs-abort timing vector properties.

Cheap harness idea:

- Feed `execute_inner` already-typed tiny transactions.
- Use command sequences biased toward:
  - malformed references
  - repeated gas use
  - duplicate object operands
  - coin overflows and insufficient balances
- Assertions:
  - no panic
  - error index matches failing command
  - `timings.len()` equals executed-command count
  - success path records post-execution state without losing generated IDs

Crash scenarios:

- command failure path dropping `context` and saving runtime metadata in an invalid borrow state
- malformed command sequences that cause invariant failures before error indexing is attached
- post-loop success path where runtime snapshots and `context.finish()` disagree about object bookkeeping
- gas-sensitive command sequences such as repeated `SplitCoins` / `MergeCoins` around near-zero balances

Crash oracle additions:

- panic during `execute_command`, failure unwinding, or `context.finish()`
- invariant failures where:
  - loaded runtime objects are missing on abort
  - generated IDs and finished results diverge
  - gas bookkeeping reaches an impossible state during post-execution finalization
- mismatch between failing command and attached command index

## Cross-Function Invariant Chain

These three functions form one continuous trust boundary:

```text
raw PTB
  ->
linkage + translation + typing
  ->
typed inputs become runtime locals/native extensions/gas
  ->
typed commands execute sequentially
  ->
runtime state is collapsed back into persisted execution results
```

The critical coupled invariants are:

1. `mod.rs::execute` must not let raw PTB/linkage inconsistencies reach the interpreter.
2. `Context::new` must preserve one coherent runtime model for objects, gas, and tx context.
3. `execute_inner` must preserve failure metadata and post-execution persistence ordering.

If you fuzz these in isolation, keep those couplings in mind. A good harness models one seam faithfully rather than stubbing away the assumptions established by the previous seam.

## Best First Harness to Write

For a low-end machine, the first harness should target `Context::new`.

Why:
- It is narrower than full interpretation.
- It contains high-value semantic branching around gas and object materialization.
- It exposes many invariants without requiring long command sequences.
- It is close enough to execution to catch real state-model bugs.

Suggested progression:

1. `Context::new` property test harness
2. `Context::new` crash harness biased toward invariant-violation inputs
3. `execute_inner` micro-sequence property test harness
4. `execute_inner` crash harness for abort/finalization edge cases
5. Minimal `cargo-fuzz` harness only after one of the above produces useful structure

## Existing In-Tree Patterns to Reuse

- `crates/transaction-fuzzer`
  - Use as a model for deterministic, bounded local fuzz/property tests.
- `external-crates/move/crates/bytecode-verifier-libfuzzer`
  - Use as a model for a minimal `cargo-fuzz` harness once a single narrow function is selected.

Do not start by trying to fuzz all of `sui-adapter-latest` end-to-end. On a weak machine that will spend most of the budget on setup and compilation noise rather than the Phase 1 state transitions you actually care about.
