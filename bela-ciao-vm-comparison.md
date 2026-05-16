Previous Sui VM vs Bela-Ciao VM
================================

This note compares the older Sui programmable transaction execution path with
the newer Bela-Ciao implementation.

The goal here is clarity, not marketing language.

Terms
-----

Before comparing the two designs, it helps to define what the code is actually
processing.

In this note:

- `transaction` means one Sui transaction being executed.
- `programmable transaction` or `PTB` means the programmable part inside that
  transaction.
- `commands` means the list of actions inside the PTB.
- `inputs` means the arguments that those commands can use.

Those PTB inputs can include:

- object references
- pure byte values
- receiving objects
- withdrawal arguments

Those PTB commands can include:

- `MoveCall`
- `TransferObjects`
- `SplitCoins`
- `MergeCoins`
- `Publish`
- `Upgrade`

So when this write-up says "input", it does not mean "all external user input"
in a vague sense. It specifically means one argument made available to the
programmable transaction before the commands begin running.


## 1. The Older Implementation
---------------------------

The older Sui VM path handled a programmable transaction more directly.

At a high level, it did this:

1. take the raw programmable transaction
2. split it into `inputs` and `commands`
3. load those inputs into an execution context
4. resolve packages and types while processing commands
5. execute the commands one by one
6. save the results

The legacy execution entrypoint can be seen here:

- `sui-execution/v3/sui-adapter/src/programmable_transactions/execution.rs:84`

In that file, the old path is still visible. If
`enable_ptb_execution_v2()` is on, execution jumps to the new static path. If
not, it stays on the old one:

- `sui-execution/v3/sui-adapter/src/programmable_transactions/execution.rs:96`

The raw programmable transaction is split into `inputs` and `commands` here:

- `sui-execution/v3/sui-adapter/src/programmable_transactions/execution.rs:145`

Then an `ExecutionContext` is created directly from the raw PTB inputs:

- `sui-execution/v3/sui-adapter/src/programmable_transactions/execution.rs:146`

The main legacy runtime state container is:

- `sui-execution/v3/sui-adapter/src/programmable_transactions/context.rs:82`

That context holds most of the important per-transaction state:

- the gas coin
- the input arguments
- the command results
- borrow tracking
- linkage state
- newly published packages
- user events

You can see those fields here:

- `sui-execution/v3/sui-adapter/src/programmable_transactions/context.rs:83`
- `sui-execution/v3/sui-adapter/src/programmable_transactions/context.rs:107`

The legacy command loop is simple:

- iterate through PTB commands
- match the raw command variant
- execute it
- record success or abort timing

That loop is here:

- `sui-execution/v3/sui-adapter/src/programmable_transactions/execution.rs:158`

And raw command dispatch starts here:

- `sui-execution/v3/sui-adapter/src/programmable_transactions/execution.rs:199`


Old path diagram
----------------

```text
raw programmable transaction
  |
  |-- inputs
  |-- commands
  v
ExecutionContext
  |
  |-- hold gas, inputs, results
  |-- resolve packages as needed
  |-- resolve types as needed
  |-- track borrows and object usage
  v
execute raw commands one by one
  |
  v
save results
```


## What the old design was really doing
------------------------------------

The old path was doing several jobs at once inside one broad runtime layer.

It was:

- storing transaction state
- loading PTB arguments
- resolving package information
- resolving type information
- enforcing usage rules
- executing commands

This made the design more direct, but also more mixed together.

For example, the legacy execution context creates and carries a `LinkageView`
for package resolution:

- `sui-execution/v3/sui-adapter/src/programmable_transactions/context.rs:164`

Type layout resolution also relied on legacy linkage helpers:

- `sui-execution/v3/sui-adapter/src/type_layout_resolver.rs:19`
- `sui-execution/v3/sui-adapter/src/type_layout_resolver.rs:42`

So in the old model, the VM was closer to:

"start executing the programmable transaction, and resolve more details as
execution goes forward."


## 2. Where the Older Design Became Awkward
----------------------------------------

The older design worked, but it had a few structural limitations.

First, it stayed very close to the raw PTB shape.

That means the executor had to keep track of a lot of meaning during execution:

- what each argument is
- how it should be used
- what package or type it belongs to
- whether it is currently borrowed
- what command results exist so far

You can see part of that bookkeeping in the legacy context fields:

- `inputs`
- `results`
- `borrowed`
- `per_command_by_value_shared_objects`

at:

- `sui-execution/v3/sui-adapter/src/programmable_transactions/context.rs:109`
- `sui-execution/v3/sui-adapter/src/programmable_transactions/context.rs:114`
- `sui-execution/v3/sui-adapter/src/programmable_transactions/context.rs:115`
- `sui-execution/v3/sui-adapter/src/programmable_transactions/context.rs:118`

Second, package resolution and execution were closely tied together.

The system was not cleanly saying:

"first build the package-resolution context for this transaction, then execute."

It was more like:

"build enough context to keep execution going."

Third, type handling was less explicit as a separate internal representation.

The old path had helpers and runtime conversion logic, but not the same
compiler-like split into:

- loading
- linkage analysis
- typing
- verification
- execution


## 3. Bela-Ciao: The New Shape
---------------------------

Bela-Ciao changes the model from:

"execute the raw programmable transaction"

to:

"translate the programmable transaction into a better internal form, then
execute that."

The main new entrypoint is:

- `sui-execution/latest/sui-adapter/src/static_programmable_transactions/mod.rs:38`

Its flow is explicit:

1. build a package store with caching
2. compute package/type linkage for the transaction
3. create a VM view for that linkage
4. translate the raw PTB into an internal form
5. type-check and verify that internal form
6. execute typed commands

You can see those stages here:

- package store setup:
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/mod.rs:52`
- linkage computation:
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/mod.rs:53`
- linkage-aware VM creation:
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/mod.rs:59`
- loading translation:
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/mod.rs:79`
- typing and verification:
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/mod.rs:91`
- execution:
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/mod.rs:93`


New path diagram
----------------

```text
raw programmable transaction
  |
  v
loading::translate
  |
  v
linkage analysis
  |
  v
typed transaction form
  |
  v
verification passes
  |
  v
typed interpreter
  |
  v
save results
```


## 4. What Changed in Bela-Ciao
----------------------------

## 4.1 Package loading and caching
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

In Bela-Ciao, package caching is a central part of the design, not just a
background optimization.

The package loading layer is:

- `sui-execution/latest/sui-adapter/src/data_store/cached_package_store.rs`

Its role is described directly in the file:

- `sui-execution/latest/sui-adapter/src/data_store/cached_package_store.rs:18`

That code does two important things:

- if a package was published in the current transaction, return that directly
- otherwise ask the runtime to resolve and cache the package

That logic is here:

- `sui-execution/latest/sui-adapter/src/data_store/cached_package_store.rs:61`

This is a real design change.

The old path used package and linkage helpers inside the execution context. The
new path makes package loading more explicit and reusable before command
execution.

Simple summary:

- old path: package resolution was more execution-driven
- Bela-Ciao: package resolution is more pipeline-driven


## 4.2 Types are made explicit earlier
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

One of the clearest changes is that Bela-Ciao builds a typed internal
transaction form.

The typed transaction AST is here:

- `sui-execution/latest/sui-adapter/src/static_programmable_transactions/typing/ast.rs:20`

That AST explicitly separates:

- object inputs
- pure inputs
- receiving inputs
- withdrawal inputs
- typed commands
- typed command results

Examples:

- object input type:
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/typing/ast.rs:42`
- command type:
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/typing/ast.rs:108`
- command variants:
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/typing/ast.rs:124`

The loading pass converts raw PTB arguments into this more structured form:

- `sui-execution/latest/sui-adapter/src/static_programmable_transactions/loading/translate.rs:21`

For example, object inputs are read from storage and their Move types are loaded
before execution:

- `sui-execution/latest/sui-adapter/src/static_programmable_transactions/loading/translate.rs:85`
- `sui-execution/latest/sui-adapter/src/static_programmable_transactions/loading/translate.rs:92`

So Bela-Ciao moves meaning earlier.

Instead of the executor constantly discovering what things are while it runs, it
tries to decide that earlier and store the result in typed data structures.


4.3 Package-to-package execution is analyzed before execution
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

In the new design, package relationships are worked out before the interpreter
starts.

That logic lives in:

- `sui-execution/latest/sui-adapter/src/static_programmable_transactions/linkage/analysis.rs`

The analyzer itself starts here:

- `sui-execution/latest/sui-adapter/src/static_programmable_transactions/linkage/analysis.rs:27`

It computes package linkage for:

- function calls
- publication
- upgrade
- types appearing in inputs

Examples:

- call linkage:
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/linkage/analysis.rs:46`
- input type resolution linkage:
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/linkage/analysis.rs:79`
- publication linkage:
  `sui-execution/latest/sui-adapter/src/static_programmable_transactions/linkage/analysis.rs:65`

This matters because one programmable transaction can touch several packages at
once:

- it can call a function from package A
- use an object whose type is defined in package B
- upgrade or publish package C

Bela-Ciao builds the package-resolution context for that transaction before
running commands.

That package-aware execution setup is wired into the main Bela-Ciao entrypoint:

- `sui-execution/latest/sui-adapter/src/static_programmable_transactions/mod.rs:53`
- `sui-execution/latest/sui-adapter/src/static_programmable_transactions/mod.rs:59`


## 4.4 Verification becomes a dedicated stage
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

The old model had safety checks inside the runtime flow.

Bela-Ciao turns several of them into an explicit verification step over the
typed transaction.

The verifier entrypoint is:

- `sui-execution/latest/sui-adapter/src/static_programmable_transactions/typing/verify/mod.rs:17`

It runs separate checks for:

- input arguments
- move functions
- memory safety
- drop safety
- private entry arguments

Those passes are called here:

- `sui-execution/latest/sui-adapter/src/static_programmable_transactions/typing/verify/mod.rs:21`

This is another major design change.

Instead of putting as much burden as possible on the execution context, the new
path tries to reject or shape bad states earlier.


4.5 The interpreter runs typed commands, not raw PTB commands
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

The Bela-Ciao interpreter is here:

- `sui-execution/latest/sui-adapter/src/static_programmable_transactions/execution/interpreter.rs`

Execution starts here:

- `sui-execution/latest/sui-adapter/src/static_programmable_transactions/execution/interpreter.rs:29`

The typed transaction is unpacked here:

- `sui-execution/latest/sui-adapter/src/static_programmable_transactions/execution/interpreter.rs:73`

Then commands are processed one by one:

- `sui-execution/latest/sui-adapter/src/static_programmable_transactions/execution/interpreter.rs:99`

And command dispatch starts here:

- `sui-execution/latest/sui-adapter/src/static_programmable_transactions/execution/interpreter.rs:143`

This is important because the interpreter is no longer reading the raw PTB
format directly.

It is reading a typed internal command form.

That means translation and verification have already happened.

So the new interpreter is working on a cleaner representation than the old
interpreter did.


5. A Side-by-Side View
----------------------

Here is the short version.

Old path:

- take the raw programmable transaction
- load its inputs into a runtime context
- resolve types and packages while executing
- run raw commands directly

Bela-Ciao:

- take the raw programmable transaction
- translate it into a structured internal form
- compute package linkage up front
- type-check and verify that internal form
- run typed commands

Another compact diagram:

```text
Old
---
raw PTB -> execution context -> raw command execution -> results

Bela-Ciao
---------
raw PTB -> translation -> linkage -> typed form -> verification -> execution -> results
```


6. Why Bela-Ciao Is Cleaner
---------------------------

It is cleaner mainly because it separates jobs that used to be more mixed
together.

The old model mixed:

- transaction-state handling
- package resolution
- type resolution
- safety enforcement
- command execution

The new model splits those into clearer layers:

- loading
- linkage analysis
- typing
- verification
- execution

That split is visible in the module layout:

- `sui-execution/latest/sui-adapter/src/static_programmable_transactions/mod.rs:30`

This also explains the docs language about future Move features.

The code is now organized so that a future feature can more naturally belong to
one phase:

- loading change
- linkage change
- typing rule
- verifier rule
- interpreter behavior

instead of being forced into one large execution context.


7. Final Summary
----------------

The previous Sui PTB VM path was more direct.

It took the raw programmable transaction and executed it through a large runtime
context that also handled package resolution, type handling, borrow tracking,
and command semantics.

Bela-Ciao changes that by inserting a more explicit processing pipeline before
execution.

It:

- loads and classifies transaction arguments earlier
- computes package relationships earlier
- builds a typed internal transaction form
- runs verifier passes before execution
- executes typed commands instead of raw PTB commands

So the core difference is simple:

- old path: execution-first
- Bela-Ciao: prepare-first, then execute

That is the main reason the new design is easier to extend and easier to reason
about.
