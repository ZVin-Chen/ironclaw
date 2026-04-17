---
name: create-test-env
version: 0.1.0
description: Create an isolated IronClaw test environment with independent base directory, PostgreSQL database, and pgvector extension.
activation:
  keywords:
    - create test env
    - spin up instance
    - new ironclaw environment
    - isolated test
    - multi instance
    - test environment
    - throwaway env
  patterns:
    - "create.*test.*env"
    - "spin.*up.*instance"
    - "new.*ironclaw.*environment"
    - "isolated.*test"
    - "multi.*instance"
    - "throwaway.*env"
  exclude_keywords:
    - docker test
    - production deploy
    - cloud
  max_context_tokens: 3000
---

# Create Isolated Test Environment

Create a fully independent IronClaw instance for testing. Each environment gets its own base directory, .env config, PostgreSQL database, and ports — completely isolated from the main instance.

**Placeholder convention:** Throughout this skill, `{name}`, `{db_name}`, `{gateway_port}`, `{http_port}`, `{host}`, `{port}`, `{user}` are placeholders. Substitute them with actual computed values when executing commands.

## Prerequisites

- PostgreSQL running locally (or reachable via a configured `DATABASE_URL`)
- `psql` and `createdb` in PATH (or supply the SQL manually)
- Main IronClaw environment configured at `~/.ironclaw/` (to inherit LLM settings)

## Steps

Follow these steps in order. Use the user's input to derive the environment name.

### 1. Parse Environment Name

Extract a short name from the user's request (e.g. "test1", "my-feature", "staging").

Sanitize the name:
- Lowercase only
- Replace spaces and non-alphanumeric characters with hyphens
- Trim to 32 characters max

### 2. Check for Conflicts

Check if `~/.ironclaw-{name}` directory already exists:

```bash
ls -la ~/.ironclaw-{name}/ 2>/dev/null
```

If it exists, ask the user: "Environment `~/.ironclaw-{name}` already exists. Overwrite it or choose a different name?" Do NOT proceed without confirmation.

### 3. Scan Ports

Scan all existing IronClaw environments for used ports:

```bash
grep -h "GATEWAY_PORT\|HTTP_PORT" ~/.ironclaw*/.env 2>/dev/null || echo "no existing envs"
```

Collect all `GATEWAY_PORT` and `HTTP_PORT` values into sets.

Allocation algorithm:
- Default gateway base: 3000
- Default HTTP base: 8080
- Increment both by 1 until finding a pair where neither gateway port nor HTTP port is in the used sets
- Verify the chosen ports are free: `python3 -c "import socket; s=socket.socket(); s.settimeout(1); result=s.connect_ex(('127.0.0.1', {gateway_port})); s.close(); print('USED' if result==0 else 'FREE')"`

If the TCP check says USED, increment and try again.

### 4. Read LLM Config from Main Environment

Read `~/.ironclaw/.env` and extract all LLM-related variables. These are the common variables to copy (if they exist):

```
LLM_BACKEND
NEARAI_API_KEY NEARAI_SESSION_TOKEN NEARAI_BASE_URL NEARAI_MODEL
OPENAI_API_KEY OPENAI_MODEL
ANTHROPIC_API_KEY ANTHROPIC_MODEL ANTHROPIC_BASE_URL
OLLAMA_BASE_URL OLLAMA_MODEL
LLM_BASE_URL LLM_MODEL LLM_API_KEY LLM_REQUEST_TIMEOUT_SECS
GEMINI_API_KEY GEMINI_MODEL
MINIMAX_API_KEY MINIMAX_MODEL
```

IronClaw supports additional providers (tinfoil, bedrock, openai_codex, openai_compatible). If the main environment uses any of these, copy their provider-specific vars too. When in doubt, copy all vars matching `NEARAI_*`, `OPENAI_*`, `ANTHROPIC_*`, `OLLAMA_*`, `GEMINI_*`, `MINIMAX_*`, `LLM_*`, or the provider name.

Also read the `DATABASE_URL` to extract the PostgreSQL host, port, and user for the new database.

```bash
cat ~/.ironclaw/.env 2>/dev/null || echo "no main env"
```

Parse `DATABASE_URL` to extract connection params. Example formats:
- `postgres://localhost/ironclaw` → host=localhost, user=current OS user, port=5432
- `postgres://user:pass@host:5432/dbname` → extract all components

If the URL cannot be parsed, ask the user for:
- PostgreSQL host (default: `localhost`)
- PostgreSQL port (default: `5432`)
- PostgreSQL user (default: current OS user)
- PostgreSQL password (if needed)

### 5. Create Directory

```bash
mkdir -p ~/.ironclaw-{name}
```

### 6. Generate .env File

Write `~/.ironclaw-{name}/.env` with the following content. Replace placeholders with computed values.

Generate the database name: lowercase the environment name, replace hyphens with underscores, prefix with `ironclaw_`. Example: `my-feature` → `ironclaw_my_feature`.

Write the .env using the Write tool (not bash echo) to ensure proper quoting:

```
DATABASE_BACKEND="postgres"
DATABASE_URL="postgres://{user}@{host}:{port}/ironclaw_{db_name}"
DATABASE_POOL_SIZE="5"
ONBOARD_COMPLETED="true"
HEARTBEAT_ENABLED="false"
SANDBOX_ENABLED="false"
GATEWAY_HOST="127.0.0.1"
GATEWAY_PORT="{gateway_port}"
HTTP_HOST="127.0.0.1"
HTTP_PORT="{http_port}"
{inherited LLM vars — one per line, copied verbatim from main env}
```

If the main environment has no LLM config, add a comment:
```
# WARNING: No LLM configuration found in main environment.
# Add your LLM_BACKEND and API key settings below.
```

### 7. Create Database

First, create the database:

```bash
createdb ironclaw_{db_name}
```

If `createdb` is not found or fails, print the manual commands and continue:

```
Could not create database automatically. Run these commands manually:

    createdb ironclaw_{db_name}
    psql ironclaw_{db_name} -c "CREATE EXTENSION IF NOT EXISTS vector;"
```

Then enable pgvector (required for workspace memory vector search):

```bash
psql ironclaw_{db_name} -c "CREATE EXTENSION IF NOT EXISTS vector;"
```

If this fails, print a warning but do not abort — the env is still usable once the DB and extension are set up.

### 8. Verify and Print Summary

Verify the environment:

```bash
# Check .env exists
cat ~/.ironclaw-{name}/.env

# Check database exists
psql -l | grep ironclaw_{db_name}

# Check pgvector is enabled
psql ironclaw_{db_name} -c "SELECT extname FROM pg_extension WHERE extname='vector';"
```

Print the summary:

```
Test environment '{name}' created!

   Directory:   ~/.ironclaw-{name}
   Database:    ironclaw_{db_name}
   Gateway:     http://127.0.0.1:{gateway_port}/
   HTTP webhook: http://127.0.0.1:{http_port}/

   Start:
     IRONCLAW_BASE_DIR=~/.ironclaw-{name} ironclaw run

   Tear down:
     rm -rf ~/.ironclaw-{name}
     dropdb ironclaw_{db_name}
     # or: psql -c "DROP DATABASE ironclaw_{db_name};"
```

## Common Issues

| Issue | Fix |
|-------|-----|
| `createdb: command not found` | Install PostgreSQL client tools, or run the SQL manually via `psql` |
| `createdb: permission denied` | Ask your DBA for CREATEDB privilege, or use an existing database |
| `ERROR: could not open extension control file` | Install pgvector: `brew install pgvector` (macOS), `sudo apt install postgresql-pgvector` (Ubuntu/Debian), or compile from [source](https://github.com/pgvector/pgvector#installation) |
| Port still occupied after scan | The TCP check may miss brief connections — try the next port pair |
| Main env has no LLM config | Add `LLM_BACKEND` and your API key to `~/.ironclaw-{name}/.env` manually |
