# REASONIX.md

Conary's canonical assistant guidance lives in `AGENTS.md`.

Read that contract, then route the actual task without preloading the complete
ownership map:

```
bash scripts/agent-context.sh --feature <slug>
bash scripts/agent-context.sh --path <file>
```

Use `docs/llms/README.md` only for broader orientation. This shim contains no
independent policy.
