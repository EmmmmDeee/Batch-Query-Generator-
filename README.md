# Batch-Query-Generator

An **Oathnet batch query generator** written in Rust. It accepts arbitrary text input, filters and normalizes each line (or semicolon-delimited segment) into a structured Oathnet batch query, and prints the results to stdout.

---

## Features

- Reads from **stdin** or one or more **files**
- Splits input on newlines and semicolons
- Discards blank lines and symbol-only lines
- Normalizes whitespace and strips unsupported characters
- Assigns every query a **unique UUID** and a sequential index
- Outputs structured `BATCH_QUERY { … }` records ready for Oathnet

---

## Building

```bash
cargo build --release
```

The binary is placed at `target/release/batch-query-generator`.

---

## Usage

### Read from stdin

```bash
echo "find all users where age > 18
get posts by author:john
list active sessions; fetch user profile id=42" | ./target/release/batch-query-generator
```

### Read from a file

```bash
./target/release/batch-query-generator queries.txt
```

### Sample output

```
Generated 4 Oathnet batch queries:
BATCH_QUERY { id: "e951a3a0-64b5-455b-93ab-4a3f1cc3018c", index: 0, query: "find all users where age > 18" }
BATCH_QUERY { id: "bbf1be58-bfa8-436c-a302-7f12ab23e04f", index: 1, query: "get posts by author:john" }
BATCH_QUERY { id: "1b6668e9-f4b8-48d1-b8bd-7ef28fbb83cb", index: 2, query: "list active sessions" }
BATCH_QUERY { id: "1536c99c-e85c-4115-9ac2-ca3575972a66", index: 3, query: "fetch user profile id=42" }
```

---

## Running tests

```bash
cargo test
```

---

## Project structure

```
src/
  lib.rs   – core filtering and query-generation logic (with unit tests)
  main.rs  – CLI entry point (stdin / file arguments)
Cargo.toml
```
