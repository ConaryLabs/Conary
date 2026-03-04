# Context7: External Documentation

Before guessing at library APIs, check Context7 for up-to-date docs.

## When to Use

- Implementing with external libraries (check current API first)
- Debugging library errors (verify correct usage)
- Uncertain about any library API (don't guess, look it up)
- Version-specific behavior matters

## Workflow

```
resolve-library-id(libraryName="tokio", query="async runtime spawn tasks")
query-docs(libraryId="/tokio-rs/tokio", query="how to spawn async tasks")
```

## When NOT to Use

- Rust standard library features
- Confident in the API from recent usage
- Simple operations with well-known patterns
