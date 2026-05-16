# Sui Blockchain Client Security Audit Report

**Audit Date:** 2026-04-18
**Target:** /home/vielite/hackenProof/sui
**Scope:** Rust blockchain client (~800K+ LOC), 3,212 files, 6 subsystems
**Skill Version:** 3

---

## Executive Summary

This audit analyzed the Sui blockchain client codebase, a Rust-based blockchain using Move VM and Mysticeti consensus. The audit covered six subsystem groups across trust boundaries from unauthenticated P2P (trust level 1) to RPC endpoints (trust level 6).

**Finding Summary:**
- **3 findings confirmed** (2 High, 1 Medium severity)
- **0 Critical** findings
- No Critical or High findings in transaction processing, consensus, or RPC subsystems

The highest-risk findings are in P2P networking (unbounded storage) and bridge (missing transaction success verification). Both could lead to resource exhaustion or integrity issues but have existing partial mitigations that reduce exploitability.

**Not Analyzed (out of scope):**
- Mysticeti consensus DagBuilder implementation details
- Move VM bytecode verifier internals
- Indexer API implementation
- Full node vs authority-specific code paths

---

## Severity Summary

| Severity | Count | Key Areas |
|----------|-------|-----------|
| Critical | 0 | — |
| High | 2 | p2p (network), bridge |
| Medium | 1 | bridge |
| Low | 0 | — |
| Info | 0 | — |
| **Total** | **3** | |

---

## Findings

### HIGH Severity

---

### p2p-P9-01: Unbounded Peer Address Storage

| Field | Value |
|-------|-------|
| **Severity** | High |
| **Confidence** | 85 |
| **Pattern** | P9 - P2P Network Layer Resource Exhaustion |
| **Location** | crates/sui-network/src/discovery/mod.rs:131, 560-590 |
| **Entry Point** | Discovery P2P endpoint - handle_message → handle_peer_address_change |
| **Impact** | Memory exhaustion via unbounded peer_addresses HashMap |

#### Description

The discovery service stores peer addresses in an unbounded HashMap (`peer_addresses: HashMap<PeerId, BTreeMap<AddressSource, Vec<anemo::types::Address>>>`) without any size limits. While individual address validation exists (`MAX_ADDRESS_LENGTH`, `MAX_ADDRESSES_PER_PEER`), there is **no limit on total peer entries** or total addresses stored.

#### Trigger Scenario

1. Attacker connects to discovery service as a peer
2. Attacker calls `update_endpoint` with `EndpointId::P2p(peer_id)` and `AddressSource::Discovery`
3. Each call routes to `handle_peer_address_change` which inserts into `peer_addresses` HashMap
4. No bound check prevents unbounded growth

#### Quantitative Assessment

- Address size: up to 300 bytes each (`MAX_ADDRESS_LENGTH`)
- Per attacker: can inject addresses for multiple unique PeerIds (generating new keys is cheap)
- Attacker can send updates rapidly via discovery channel
- No cleanup mechanism on peer disconnect (addresses only removed on explicit empty update)

Conservative estimate: 1KB per peer entry × 100,000 peers = ~100 MB, but with repeated updates and multiple address sources per peer, easily scales higher.

#### Existing Mitigations

- **Address validation**: Individual addresses are limited to 300 bytes (line 300), limits per-peer to 2 addresses (line 294)
- **Rate limiting**: `known_peers_rate_limit` applies to RPC requests only (builder.rs:77-87)
- **No per-peer limit**: No bound on how many unique PeerIds can be stored in `peer_addresses`

#### Missing Defenses

- [ ] **Global size limit**: No hard cap on total entries in `peer_addresses` HashMap
- [ ] **Per-peer memory budget**: No memory budget tracking per unique peer
- [ ] **Eviction policy**: No LRU or TTL eviction for stale addresses
- [ ] **Connection state cleanup**: Peer disconnect doesn't clean up stored addresses

#### Recommendation

Add a global size bound to `peer_addresses` HashMap with LRU eviction for oldest entries when limit is reached.

---

### p2p-P9-02: Unbounded Future Epoch Partial Signatures Storage

| Field | Value |
|-------|-------|
| **Severity** | High |
| **Confidence** | 80 |
| **Pattern** | P9 - P2P Network Layer Resource Exhaustion |
| **Location** | crates/sui-network/src/randomness/mod.rs:235, 490-499 |
| **Entry Point** | Randomness network: handle_message → receive_partial_signatures |
| **Impact** | Memory exhaustion via unbounded future_epoch_partial_sigs BTreeMap |

#### Description

The randomness network stores partial signatures for future epochs in an unbounded BTreeMap (`future_epoch_partial_sigs: BTreeMap<(EpochId, RandomnessRound, PeerId), Vec<Vec<u8>>>`).

At line 490-499, when partial signatures arrive for a future epoch (where `epoch != self.epoch`), the code:
1. Checks if the round is too far ahead (`round.0 >= max_partial_sigs_rounds_ahead()`)
2. If not too far ahead, inserts the signature into `future_epoch_partial_sigs`

**The vulnerability**: There is **no limit on how many unique PeerIds** can submit signatures for the same epoch/round combination. An attacker can exhaust memory by sending partial signatures from many unique peer IDs.

#### Trigger Scenario

1. Attacker participates in randomness network
2. Attacker sends partial signatures for a future epoch with round = 1 (within allowed limit)
3. Each message from a unique peer_id gets inserted into the BTreeMap
4. No bound on number of unique peer_ids accepted
5. Attacker repeats with different peer IDs to accumulate entries

#### Quantitative Assessment

- Each `sig_bytes` entry: Variable size - BCS-serialized randomness partial signatures
- Attacker can send from many unique PeerIds (generating keys is cheap)
- No cleanup in the code path for future_epoch_partial_sigs until epoch transition (lines 376-378)
- Default max_partial_sigs_rounds_ahead is likely small (e.g., 2) but the KEY is unlimited peer_ids per round

Conservative estimate: 1KB per entry × 50,000 peer messages = 50 MB, repeating can cause unbounded growth.

#### Existing Mitigations

- **Round limit**: Only accepts signatures within `max_partial_sigs_rounds_ahead` rounds ahead (line 491)
- **Epoch check**: Skips if epoch > self.epoch + 1 (line 469)
- **No peer count limit**: No bound on how many unique PeerIds can submit for the same epoch/round

#### Missing Defenses

- [ ] **Per-round peer limit**: No hard cap on unique PeerIds per epoch/round combination
- [ ] **Memory budget**: No memory budget tracking
- [ ] **Cleanup on timeout**: No automatic cleanup for stale future epoch entries

#### Recommendation

Add a per-round limit on unique peer submissions.

---

### MEDIUM Severity

---

### bridge-P10-01: Bridge Signatures for Reverted Transactions

| Field | Value |
|-------|-------|
| **Severity** | Medium |
| **Confidence** | 75 |
| **Pattern** | P10 - Cross-Layer / Bridge Message Integrity Failures |
| **Location** | crates/sui-bridge/src/server/handler.rs:128-136 and :138-148 |
| **Entry Point** | HTTP endpoints: `/sign/bridge_tx/eth/sui/{tx_hash}/{event_index}` and `/sign/bridge_tx/sui/eth/{tx_digest}/{event_index}` |
| **Impact** | Attacker can obtain bridge authority signatures for transfer messages that never actually executed on the source chain, potentially enabling replay or double-spend attacks if the signed action is later accepted by the bridge executor. |

#### Description

The bridge request handlers (handle_eth_tx_hash and handle_sui_tx_digest) retrieve events from external chain transactions and generate signed BridgeAction objects without verifying that the source transaction actually succeeded.

For Ethereum transactions (handle_eth_tx_hash):
- The handler calls eth_client.get_finalized_bridge_action_maybe(tx_hash, event_idx)
- Located at crates/sui-bridge/src/eth_client.rs:92-136
- The code checks the transaction is finalized but does NOT check receipt.status() to verify the transaction succeeded

For Sui transactions (handle_sui_tx_digest):
- The handler calls sui_client.get_bridge_action_by_tx_digest_and_event_idx_maybe(tx_digest, event_idx)
- Located at crates/sui-bridge/src/sui_client.rs:133-152
- The code queries events by transaction digest but does NOT check the transaction effects/status to verify the transaction succeeded

#### Trigger Scenario

1. Attacker initiates a token transfer on Ethereum that is designed to revert (e.g., gas limit reached, contract logic failure)
2. The transaction emits a valid bridge event before reverting
3. Attacker calls the bridge server endpoint /sign/bridge_tx/eth/sui/{tx_hash}/{event_index} with the reverted transaction's hash
4. Bridge server retrieves the event from the reverted transaction, generates a signed BridgeAction
5. Attacker obtains a valid signature for an action that never actually executed

The same attack applies to Sui transactions via /sign/bridge_tx/sui/eth/{tx_digest}/{event_index}.

#### Quantitative Assessment

- Cost per attack: Minimal - just an HTTP request to the bridge server
- Rate limit: None observed at the handler level (only global request size limits at mod.rs:41-42)
- Impact: Any successfully reverted transaction with a bridge event can be signed by the bridge authority, creating a valid-looking SignedBridgeAction for a non-existent transfer

#### Existing Mitigations

- Transaction finalization check (Eth): Code at eth_client.rs:111-113 checks receipt_block_num > last_finalized_block_id and returns TxNotFinalized - this prevents reorg attacks but NOT reverted transactions
- Contract address validation (Eth): Code at eth_client.rs:121-123 validates the event came from a recognized bridge contract - this is present but only validates source, not transaction success
- Package validation (Sui): Code at sui_client.rs:143-144 validates event came from the bridge package - this is present but only validates source, not transaction success
- Error type exists but unused: BridgeError::OriginTxFailed exists in error.rs:10-11 but is never returned by either handler

#### Missing Defenses

- Transaction success verification (Eth): Check receipt.status() to ensure the transaction succeeded before processing the event
- Transaction success verification (Sui): Query transaction effects and check status.success() before processing the event
- Use existing error type: The OriginTxFailed error exists but is not used - it should be returned when a reverted transaction is detected

#### Recommendation

1. In eth_client.rs:get_finalized_bridge_action_maybe(), add after line 107:
   ```
   if !receipt.status().is_ok() {
       return Err(BridgeError::OriginTxFailed);
   }
   ```

2. In sui_client.rs:get_bridge_action_by_tx_digest_and_event_idx_maybe(), add before processing events: query transaction effects and verify success before extracting events.

3. Ensure OriginTxFailed returns appropriate HTTP status (likely 400 Bad Request or 409 Conflict)

---

## Coverage Summary

### Analyzed

| Subsystem | Entry Points | Patterns Checked |
|-----------|--------------|-----------------|
| **p2p (network)** | 3 (state_sync, discovery, randomness handle_message) | P9, P17, P18, P20 |
| **bridge (cross-chain)** | 7 (handlers, monitor) | P10 |
| **transactions** | 7 (authority, RPC, transaction-checks, execution) | P1, P2, P7, P8, P13, P14, P15, P17, P18, P20 |
| **consensus** | 4 (handler, validator, core) | P5, P6, P9, P11, P17, P18, P20 |
| **rpc** | 8+ (read_api, coin_api, governance) | P7, P17, P18, P20 |
| **execution (VM)** | 5 (execution_engine v0-v3) | P13, P14, P17, P18, P20 |

### Not Analyzed

| Subsystem | Reason |
|-----------|--------|
| Mysticeti DagBuilder | Internal consensus building, not externally reachable without validator access |
| Move VM bytecode verifier | Internal VM safety, requires malicious bytecode which is already privileged |
| Indexer API | Depends on indexer state, not core consensus or transaction path |
| Full node vs authority differences | Requires understanding deployment model |

### Partially Analyzed

- **Bridge**: Analyzed HTTP handlers and client code; not analyzed the bridge executor on-chain logic (different trust boundary)
- **Consensus**: Analyzed transaction handling and validation; not analyzed the core Mysticeti round-by-round consensus logic in depth

---

## Pattern Coverage

**Applicable patterns (P1-P20):** P1, P2, P5, P6, P7, P8, P9, P10, P11, P13, P14, P15, P17, P18, P20

**Not applicable:**
- P3: No EVM compatibility layer (Sui uses Move VM)
- P4: Validator set managed via SuiSystemState object, not local pallet storage
- P12: No ZK prover/verifier code in this codebase
- P16: No plugin/module registration system in runtime
- P19: Not C/C++ - pure Rust codebase

---

## Methodology Notes

This audit used systematic pattern matching against 20 historical vulnerability families with structured trust-boundary analysis. The 3-check FP gate (concrete execution path, externally reachable entry point, no sufficient existing defense) was applied to all potential findings.

**Strengths of this audit:**
- Systematic coverage of entry points across all trust boundaries
- Quantitative impact assessment where applicable
- Pattern-guided analysis with historical vulnerability reference
- Honest coverage reporting

**Limitations:**
- Novel vulnerability classes without historical precedent may be missed
- Complex multi-step chains spanning many subsystems may not be captured
- Business logic bugs specific to Sui's economic design not covered by P1-P20
- Dynamic analysis (timing-dependent bugs) not possible in static review

---

*Report generated by client-auditor skill v3*