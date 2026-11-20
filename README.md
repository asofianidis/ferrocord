# ferrocord

A fast, native Discord client written in Rust.

## Why

The official client is an Electron app: high memory use, slow startup, and sluggish UI. The goal here is a lightweight native client that starts instantly and stays responsive.

## Goals

- Low memory footprint and fast cold start
- Smooth scrolling and rendering in large servers and long channel histories
- Native code end to end — no embedded browser

## Status

Early scaffolding. Nothing usable yet.

## Building

```sh
cargo run
```
