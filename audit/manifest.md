# Audit Manifest

## Codebase Overview
- Language(s): Rust (100%)
- Framework: Custom Sui blockchain (Move VM, Mysticeti consensus) - not Substrate or Cosmos SDK
- Size: ~3,212 Rust files, estimated ~800K+ lines of code
- Notable: Multi-version execution (v0-v3), native bridge to Ethereum, Move language VM, no EVM compatibility

## Applicable Patterns
**Applicable:** P1, P2, P5, P6, P7, P8, P9, P10, P11, P13, P14, P15, P17, P18, P20

**Not applicable:**
- P3: No EVM compatibility layer (pallet-evm, Frontier, geth-fork) - Sui uses Move VM
- P4: Validator set managed via SuiSystemState object, not local pallet storage
- P12: No ZK prover/verifier code in this codebase
- P16: No plugin/module registration system in runtime
- P19: Not C/C++ - pure Rust codebase

## Entry Points

| Subsystem | Trust Level | File | Line | Function |
|-----------|-------------|------|------|----------|
| transactions | 4 | crates/sui-core/src/authority.rs | 1023 | handle_transaction_deny_checks |
| transactions | 4 | crates/sui-core/src/authority.rs | 1188 | check_transaction_validity |
| transactions | 6 | crates/sui-json-rpc/src/transaction_execution_api.rs | 137 | execute_transaction_block |
| transactions | 3 | crates/sui-core/src/authority_server.rs | 562 | handle_submit_transaction |
| transactions | 3 | crates/sui-core/src/authority_client.rs | 36 | submit_transaction |
| transactions | 3 | crates/sui-transaction-checks/src/lib.rs | 76 | check_transaction_input |
| transactions | 3 | crates/sui-transaction-checks/src/lib.rs | 104 | check_transaction_input_with_given_gas |
| consensus | 5 | crates/sui-core/src/consensus_handler.rs | 1093 | handle_consensus_commit |
| consensus | 5 | crates/sui-core/src/consensus_handler.rs | 1680 | process_transactions |
| consensus | 5 | consensus/core/src/core.rs | (many) | Core consensus processing |
| consensus | 5 | crates/sui-core/src/consensus_validator.rs | 74 | validate_transactions |
| bridge | 2 | crates/sui-bridge/src/server/handler.rs | 24 | handle_eth_tx_hash |
| bridge | 2 | crates/sui-bridge/src/server/handler.rs | 32 | handle_sui_tx_digest |
| bridge | 2 | crates/sui-bridge/src/server/handler.rs | 40 | handle_sui_token_transfer |
| bridge | 2 | crates/sui-bridge/src/server/handler.rs | 48 | handle_governance_action |
| bridge | 2 | crates/sui-bridge/src/server/mod.rs | 274 | handle_eth_tx_hash |
| bridge | 2 | crates/sui-bridge/src/server/mod.rs | 326 | handle_sui_token_transfer |
| bridge | 2 | crates/sui-bridge/src/monitor.rs | 107 | handle_sui_events |
| p2p | 1 | crates/sui-network/src/state_sync/mod.rs | 814 | handle_message |
| p2p | 1 | crates/sui-network/src/discovery/mod.rs | 418 | handle_message |
| p2p | 1 | crates/sui-network/src/randomness/mod.rs | 262 | handle_message |
| rpc | 6 | crates/sui-json-rpc/src/read_api.rs | 165 | object |
| rpc | 6 | crates/sui-json-rpc/src/read_api.rs | 864 | get_transaction_block |
| rpc | 6 | crates/sui-json-rpc/src/transaction_builder_api.rs | 80 | transfer_object |
| rpc | 6 | crates/sui-json-rpc/src/coin_api.rs | 120 | get_coins |
| rpc | 6 | crates/sui-json-rpc/src/governance.rs | (many) | Staking operations |
| execution | 5 | sui-execution/latest/sui-adapter/src/execution_engine.rs | 119 | execute_transaction_to_effects |
| execution | 5 | sui-execution/v0/sui-adapter/src/execution_engine.rs | 55 | execute_transaction_to_effects |
| execution | 5 | sui-execution/v1/sui-adapter/src/execution_engine.rs | 60 | execute_transaction_to_effects |
| execution | 5 | sui-execution/v2/sui-adapter/src/execution_engine.rs | 66 | execute_transaction_to_effects |
| execution | 5 | sui-execution/v3/sui-adapter/src/execution_engine.rs | 90 | execute_transaction_to_effects |

## Subsystem Groups

### Group 1: transactions
**Trust level:** 4 (Signed transaction, fee-gated)
**Entry points:** 
- handle_transaction_deny_checks (authority.rs:1023)
- check_transaction_validity (authority.rs:1188)
- execute_transaction_block (json-rpc/transaction_execution_api.rs:137)
- handle_submit_transaction (authority_server.rs:562)
- check_transaction_input (sui-transaction-checks/src/lib.rs:76)
**Pattern files:** client-attack-patterns-1.md (P1), client-attack-patterns-2.md (P5, P7, P8), client-attack-patterns-4.md (P13, P14), client-attack-patterns-5.md (P17, P18, P20)
**Priority:** high - high trust boundary but large code volume

### Group 2: consensus
**Trust level:** 5 (Validator-only, stake-gated)
**Entry points:**
- handle_consensus_commit (consensus_handler.rs:1093)
- process_transactions (consensus_handler.rs:1680)
- validate_transactions (consensus_validator.rs:74)
- Core consensus (consensus/core/src/core.rs)
**Pattern files:** client-attack-patterns-2.md (P5, P6), client-attack-patterns-3.md (P9, P10, P11), client-attack-patterns-5.md (P17, P18, P20)
**Priority:** high - validator-only but critical to security

### Group 3: bridge (cross-chain)
**Trust level:** 2 (Cross-chain - external chain as trust root)
**Entry points:**
- handle_eth_tx_hash (bridge/server/handler.rs:24)
- handle_sui_tx_digest (bridge/server/handler.rs:32)
- handle_sui_token_transfer (bridge/server/handler.rs:40)
- handle_governance_action (bridge/server/handler.rs:48)
- handle_sui_events (bridge/monitor.rs:107)
**Pattern files:** client-attack-patterns-3.md (P10)
**Priority:** high - lowest trust boundary, external chain interaction

### Group 4: p2p (network)
**Trust level:** 1 (Unauthenticated P2P - any peer can trigger)
**Entry points:**
- handle_message (state_sync/mod.rs:814)
- handle_message (discovery/mod.rs:418)
- handle_message (randomness/mod.rs:262)
**Pattern files:** client-attack-patterns-3.md (P9), client-attack-patterns-5.md (P17, P18, P20)
**Priority:** high - lowest trust boundary, untrusted input

### Group 5: rpc
**Trust level:** 6 (Operator/user-facing)
**Entry points:**
- object (read_api.rs:165)
- get_transaction_block (read_api.rs:864)
- transfer_object (transaction_builder_api.rs:80)
- get_coins (coin_api.rs:120)
- Various governance RPCs
**Pattern files:** client-attack-patterns-2.md (P7), client-attack-patterns-5.md (P17, P18, P20)
**Priority:** medium - operator-facing, some fee gates

### Group 6: execution (VM)
**Trust level:** 5 (Internal, but transaction-triggered)
**Entry points:**
- execute_transaction_to_effects (execution_engine.rs - multiple versions)
**Pattern files:** client-attack-patterns-4.md (P13, P14), client-attack-patterns-5.md (P17, P18, P20)
**Priority:** medium - internal but directly handles transaction results

## Cross-Subsystem Interactions

| From | To | File | Line | Notes |
|------|-----|------|------|-------|
| rpc | transactions | sui-json-rpc/src/transaction_execution_api.rs | 152 | execute_transaction_block calls transaction_orchestrator.execute_transaction_block |
| transactions | consensus | sui-core/src/authority_server.rs | 1044 | handle_submit_to_consensus_for_position |
| consensus | execution | sui-core/src/consensus_handler.rs | 1241 | process_transactions calls execution scheduler |
| transactions | bridge | sui-bridge/src/sui_client.rs | 269 | execute_transaction_block_with_effects (for bridging) |
| p2p | transactions | sui-network/src/state_sync/mod.rs | 814 | handle_message processes checkpoint data containing transactions |
| bridge | transactions | sui-bridge/src/action_executor.rs | 450 | handle_execution_task submits bridge governance transactions |
| execution | consensus | sui-core/src/execution_driver.rs | (implicit) | Execution output feeds back to consensus |

## Agent Allocation

Recommended: 4-5 hunt agents

**Agent 1:** transactions + execution (group 1 + 6) — Priority: high
- Pattern files: client-attack-patterns-1.md, 2.md, 4.md, 5.md
- Large code volume, critical security impact

**Agent 2:** consensus (group 2) — Priority: high  
- Pattern files: client-attack-patterns-2.md, 3.md, 5.md
- Validator-only but critical

**Agent 3:** bridge (group 3) — Priority: high
- Pattern files: client-attack-patterns-3.md
- Lowest trust boundary, external chain interaction

**Agent 4:** p2p + network (group 4) — Priority: high
- Pattern files: client-attack-patterns-3.md, 5.md
- Unauthenticated input handling

**Agent 5:** rpc (group 5) — Priority: medium
- Pattern files: client-attack-patterns-2.md, 5.md
- User-facing but generally fee-gated
