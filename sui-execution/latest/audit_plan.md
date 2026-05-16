# Bela-Ciao Execution Path in `latest`

This note rewrites the earlier context in the same style as
[`/home/vielite/hackenProof/sui/bela-ciao-vm-comparison.md`](/home/vielite/hackenProof/sui/bela-ciao-vm-comparison.md).

The goal here is clarity, not audit conclusions.

## Terms

In this note:

- `transaction` means one Sui transaction being executed.
- `PTB` means the programmable transaction payload inside that transaction.
- `static PTB executor` means the Bela-Ciao path under `sui-adapter/src/static_programmable_transactions`.
- `journal` means the in-memory execution state that accumulates writes, deletes, events, and loaded objects before final effects are built.
- `effects` means the final `TransactionEffects` object committed after execution.

The main files for this path are:

- `transaction routing`: `sui-adapter/src/execution_engine.rs`
- `static PTB entrypoint`: `sui-adapter/src/static_programmable_transactions/mod.rs`
- `interpreter`: `sui-adapter/src/static_programmable_transactions/execution/interpreter.rs`
- `runtime context`: `sui-adapter/src/static_programmable_transactions/execution/context.rs`
- `execution journal`: `sui-adapter/src/temporary_store.rs`
- `gas accounting`: `sui-adapter/src/gas_charger.rs`

## 1. The Real Entrypoint

For `latest`, the outer execution entry is not the static PTB module directly.

It is:

- `sui-adapter/src/execution_engine.rs:119`

That function, `execute_transaction_to_effects`, does the outer transaction work:

1. unwrap checked inputs
2. build a `TemporaryStore`
3. build a `GasCharger`
4. build a `TxContext`
5. run the transaction
6. convert the result into final effects

The actual dispatch on transaction kind happens here:

- `sui-adapter/src/execution_engine.rs:606`

And for normal programmable transactions, routing goes here:

- `sui-adapter/src/execution_engine.rs:716`

So the relevant shape is:

```text
transaction
  v
execute_transaction_to_effects
  v
execute_transaction
  v
execution_loop
  v
SPT::execute   <-- only for programmable transactions
```

### What this means

The Bela-Ciao static executor is not the whole transaction engine.

It is one stage inside a larger execution pipeline that also handles:

- gas read charging
- system transaction routing
- journal setup
- conservation checks
- final effects construction

That matters because many correctness properties do not live inside the interpreter itself.

## 2. The Static PTB Pipeline

The Bela-Ciao PTB entrypoint is:

- `sui-adapter/src/static_programmable_transactions/mod.rs:38`

Its flow is explicit:

1. create a cached package store
2. compute linkage for this transaction
3. build a linkage-aware VM view
4. translate raw PTB into an internal form
5. type-check and verify that form
6. execute the typed transaction

You can see those stages here:

- `package store`: `sui-adapter/src/static_programmable_transactions/mod.rs:52`
- `linkage analysis`: `sui-adapter/src/static_programmable_transactions/mod.rs:53`
- `linkage-aware VM`: `sui-adapter/src/static_programmable_transactions/mod.rs:59`
- `loading translation`: `sui-adapter/src/static_programmable_transactions/mod.rs:79`
- `typing and verification`: `sui-adapter/src/static_programmable_transactions/mod.rs:91`
- `execution`: `sui-adapter/src/static_programmable_transactions/mod.rs:93`

Static path diagram:

```text
raw PTB
  v
cached package store
  v
linkage analysis
  v
linkage-aware resolution VM
  v
loading translation
  v
typed AST + verification
  v
interpreter execution
```

### What this means

The key architectural change is that execution is no longer starting from the raw PTB shape.

Instead, the runtime first builds a typed, linkage-aware internal transaction.

So when auditing Bela-Ciao, the trust boundary is not just:

```text
user input -> interpreter
```

It is:

```text
raw PTB -> linkage -> translated IR -> typed verification -> interpreter
```

## 3. What `Context::new` Is Really Doing

The runtime context is created here:

- `sui-adapter/src/static_programmable_transactions/execution/context.rs:301`

This is the point where the typed transaction stops being abstract and becomes real runtime state.

`Context::new` does all of this:

- loads object inputs into runtime values
- loads withdrawal inputs
- prepares pure inputs
- prepares receiving inputs
- creates or loads the gas coin view
- constructs native extensions
- seeds object runtime with input object metadata

Relevant anchors:

- `object input loading`: `sui-adapter/src/static_programmable_transactions/execution/context.rs:317`
- `gas coin handling`: `sui-adapter/src/static_programmable_transactions/execution/context.rs:334`
- `native extensions`: `sui-adapter/src/static_programmable_transactions/execution/context.rs:384`
- `extension construction helper`: `sui-adapter/src/adapter.rs:79`

One especially important detail is that gas may come from either:

- a real coin object
- an address-balance-backed ephemeral gas coin

That branch is handled in `Context::new`, not later.

### What this means

This is where Bela-Ciao turns "typed arguments" into "interpreter-visible locals plus object-runtime state".

So if something is wrong here, later interpreter logic may still look correct while operating on a bad runtime model.

## 4. What the Interpreter Actually Does

The interpreter entry is here:

- `sui-adapter/src/static_programmable_transactions/execution/interpreter.rs:29`

The main loop is here:

- `sui-adapter/src/static_programmable_transactions/execution/interpreter.rs:61`

The structure is simple:

1. destructure the typed transaction
2. create the execution `Context`
3. iterate through typed commands
4. execute each command
5. record timings
6. finish the context and save execution results

The command dispatch begins here:

- `sui-adapter/src/static_programmable_transactions/execution/interpreter.rs:145`

And it switches on typed commands like:

- `MoveCall`
- `TransferObjects`
- `SplitCoins`
- `MergeCoins`
- `MakeMoveVec`
- `Publish`
- `Upgrade`

That is important because the interpreter is not decoding raw PTB commands on the fly. It is executing a typed IR that has already gone through loading and verification.

Interpreter diagram:

```text
typed transaction
  |
  |-- typed inputs
  |-- typed commands
  v
Context
  |
  |-- gas local
  |-- object locals
  |-- results
  |-- native extensions
  |-- object runtime
  v
execute typed commands one by one
  v
finish context
```

### What this means

The interpreter is narrower than the old execution path.

It is mostly responsible for:

- command semantics
- local value movement/copy/borrow behavior
- VM calls
- object transfers and coin operations
- collecting runtime results

It is not responsible for the full transaction lifecycle.

## 5. What `Context::finish` Is Really Doing

The collapse back out of interpreter state begins here:

- `sui-adapter/src/static_programmable_transactions/execution/context.rs:436`

This function is doing a very important transition.

It takes:

- runtime locals
- gas local state
- object runtime state
- created IDs
- loaded child objects
- pending writes
- events

and turns them into execution results that the outer transaction engine can merge into the journal.

Important anchors:

- `draining input and gas locals`: `sui-adapter/src/static_programmable_transactions/execution/context.rs:441`
- `taking ObjectRuntime results`: `sui-adapter/src/static_programmable_transactions/execution/context.rs:522`
- `refunding reserved gas budget`: `sui-adapter/src/static_programmable_transactions/execution/context.rs:568`
- `ephemeral gas coin handling`: `sui-adapter/src/static_programmable_transactions/execution/context.rs:569`
- `serializing written objects`: `sui-adapter/src/static_programmable_transactions/execution/context.rs:582`
- `final semantic finish`: `sui-adapter/src/static_programmable_transactions/execution/context.rs:627`

### What this means

This is where the interpreter stops and chain-facing object state begins.

So this is not just cleanup code.

It is the boundary where:

- local values become object writes
- runtime-created IDs become chain-visible creations
- moved shared objects are checked against allowed behavior
- gas reservation is reconciled with final charging later

## 6. The Journal Layer: `TemporaryStore`

The transaction journal is:

- `sui-adapter/src/temporary_store.rs:40`

This structure keeps the execution state that survives outside the interpreter.

It holds:

- input objects
- mutable input refs
- loaded runtime objects
- wrapped object containers
- execution results
- generated runtime IDs
- receiving objects
- loaded per-epoch config objects

It is created here:

- `sui-adapter/src/temporary_store.rs:92`

And execution results are merged into it here:

- `sui-adapter/src/temporary_store.rs:1211`

One important piece is that non-exclusive write inputs are checked after interpreter execution and removed if unchanged:

- `sui-adapter/src/temporary_store.rs:1219`

So the journal is not just recording output blindly. It is still enforcing some post-execution correctness rules.

### What this means

`TemporaryStore` is the outer transaction state machine for object changes.

The interpreter produces candidate results.

`TemporaryStore` decides what actually survives into the final transaction journal and later effects.

## 7. Gas Charging Does Not Live Inside the Interpreter

Gas logic is concentrated in:

- `sui-adapter/src/gas_charger.rs:120`

The key points are:

- input object reads are charged before command execution: `sui-adapter/src/execution_engine.rs:365`
- gas smashing happens through `GasCharger`: `sui-adapter/src/gas_charger.rs:239`
- final storage/rebate charging happens after execution: `sui-adapter/src/gas_charger.rs:333`
- on certain failure paths, writes are dropped and gas is recomputed: `sui-adapter/src/gas_charger.rs:363`

This means gas accounting is deliberately outside the simple "run commands" model.

### What this means

A successful interpreter run is not the same thing as a successful transaction.

The outer engine can still fail or alter the result based on:

- size limits
- gas charging outcome
- storage charging outcome
- conservation checks

## 8. Final Effects Construction

The last important boundary is here:

- `sui-adapter/src/temporary_store.rs:295`

`TemporaryStore::into_effects` does the final conversion from journal state into `TransactionEffects`.

It does all of this:

- updates object versions and previous tx fields
- computes accumulator state summaries
- merges accumulator events
- adds receive-based dependencies
- computes object changes
- builds final effects

Relevant anchors:

- `version finalization`: `sui-adapter/src/temporary_store.rs:305`
- `receive dependency handling`: `sui-adapter/src/temporary_store.rs:310`
- `object changes`: `sui-adapter/src/temporary_store.rs:342`
- `final effects object`: `sui-adapter/src/temporary_store.rs:349`

Final path diagram:

```text
typed interpreter execution
  v
Context::finish
  v
TemporaryStore journal
  v
gas charging and conservation checks
  v
TemporaryStore::into_effects
  v
TransactionEffects
```

### What this means

The final chain-visible result is synthesized after the interpreter has already finished.

So if you want the real execution boundary for auditing, it is not enough to read only:

- loading
- typing
- interpreter

You also need:

- `execution_engine.rs`
- `gas_charger.rs`
- `temporary_store.rs`

## 9. The Real Architectural Split

The cleanest way to think about `latest` is this:

### Inner VM pipeline

This is the Bela-Ciao static executor:

- linkage
- translation
- typing
- verification
- typed command interpretation

### Outer transaction engine

This is the transaction wrapper around it:

- checked input unpacking
- temporary journal construction
- gas payment setup
- transaction-kind dispatch
- charging and conservation
- final effects construction

That gives a more accurate mental model than saying only:

> "Bela-Ciao is the new interpreter."

It is more precise to say:

> "Bela-Ciao is the new typed PTB execution pipeline embedded inside a larger transaction engine."

## 10. Why This Matters for Audit Context

The important audit consequence is that bugs can now live in different layers that are easier to separate conceptually.

### 1. Static executor bugs

These are bugs in:

- linkage resolution
- typed translation
- verifier assumptions
- interpreter semantics
- runtime value handling

Primary files:

- `sui-adapter/src/static_programmable_transactions/mod.rs`
- `sui-adapter/src/static_programmable_transactions/linkage/analysis.rs`
- `sui-adapter/src/static_programmable_transactions/typing/translate.rs`
- `sui-adapter/src/static_programmable_transactions/typing/verify/mod.rs`
- `sui-adapter/src/static_programmable_transactions/execution/interpreter.rs`

### 2. Boundary bugs

These are bugs where the typed executor and outer transaction engine disagree.

Examples of the boundary:

- `Context::finish` to `TemporaryStore::record_execution_results`
- gas reservation in `Context::new` vs final charging in `GasCharger`
- loaded runtime objects vs final dependency/effects generation

Primary files:

- `sui-adapter/src/static_programmable_transactions/execution/context.rs`
- `sui-adapter/src/temporary_store.rs`
- `sui-adapter/src/gas_charger.rs`

### 3. Outer engine bugs

These are bugs in:

- transaction-kind routing
- read charging order
- limit enforcement
- conservation checks
- final effects synthesis

Primary files:

- `sui-adapter/src/execution_engine.rs`
- `sui-adapter/src/temporary_store.rs`
- `sui-adapter/src/gas_charger.rs`

## Short Version

If you want a compact mental model, use this:

```text
raw transaction
  v
outer execution engine
  v
typed Bela-Ciao PTB pipeline
  v
runtime-to-journal collapse
  v
gas + conservation + effects
```

That is the version of the architecture I would use as the base context before starting vulnerability work.

If you want, the next pass can be written in the same style for just one layer:

- linkage
- typing
- interpreter
- gas/journal/effects
