# ENG-16: ABI-Aware Event & Storage Decoder

## Goal
Add ABI-aware decoding for Soroban events and storage values.

## Motivation
Current output exposes raw ScVal/XDR.
Developers need human-readable contract data.

## Scope

### sdkt-wasm
- expose ContractSpec metadata
- provide ABI type lookup

### sdkt-xdr
- decode ScVal using ABI information

### sdkt-cli
- add decode option for events/storage inspection

## Non Goals
- no contract deployment changes
- no transaction submission changes
- no sandbox implementation

## Validation
- cargo fmt
- cargo clippy
- cargo test
