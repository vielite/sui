# Progress: transactions

Status: complete
Entry points analyzed:
- crates/sui-core/src/authority.rs:1023 handle_transaction_deny_checks
- crates/sui-core/src/authority.rs:1188 check_transaction_validity  
- crates/sui-json-rpc/src/transaction_execution_api.rs:137 execute_transaction_block
- crates/sui-core/src/authority_server.rs:562 handle_submit_transaction
- crates/sui-transaction-checks/src/lib.rs:76 check_transaction_input
- crates/sui-transaction-checks/src/lib.rs:104 check_transaction_input_with_given_gas
- sui-execution/latest/sui-adapter/src/execution_engine.rs:119 execute_transaction_to_effects (and v0,v1,v2,v3 variants)

Entry points remaining: [none]
Findings written: [] - No new findings in this subsystem
Notes:
- check_transaction_validity performs early validation before transaction execution
- check_transaction_deny_checks validates transaction against deny lists and object states
- handle_submit_transaction (authority_server) processes batch transactions:
  - Batch loop iterates over each transaction (line 725-912)
  - Individual tx errors don't break batch - continue used (line 863)
  - This is correct P2 implementation - no bug found
- check_transaction_input and check_transaction_input_with_given_gas:
  - Both validate gas objects and compute gas status
  - gas_budget, gas_price, reference_gas_price used correctly
  - No early snapshot issues - gas charged during execution
- execute_transaction_to_effects (execution engine):
  - GasCharger manages gas metering
  - Storage/rebates computed after execution
  - No fee calculation issues found

Patterns checked:
- P1: Input validation panic - check_transaction_input validates objects before execution
- P2: Batch error handling - authority_server.rs batch correctly handles individual errors
- P7: RPC crash via crafted input - no panic on valid input paths
- P8: Fee calculation - gas properly calculated and charged, no originator confusion
- P14: Transaction replay - checks: get_executed_effects, transaction_executed_in_last_epoch, is_recently_finalized
- P15: Precision loss - uses u128 for intermediate calculations in gas model
- P17: unsafe blocks - in gas_charger.rs but with proper preconditions

Cross-subsystem observations:
- Transaction orchestrator calls transaction execution
- Transaction cache reader provides replay protection
- Epoch store provides protocol config and reference gas price
