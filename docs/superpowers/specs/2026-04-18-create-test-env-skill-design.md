# Create Test Environment Skill — Design Spec

**Date:** 2026-04-18
**Status:** Approved
**Scope:** Project-level IronClaw skill for creating isolated test environments

## Problem

Developers need to quickly spin up isolated IronClaw instances for testing changes, without affecting their main environment. Currently this requires manually creating directories, writing .env files, creating databases, and managing port conflicts — a multi-step error-prone process.

## Solution

A Claude Code skill (`skills/create-test-env/SKILL.md`) that guides the agent through creating a fully isolated IronClaw environment with one command.

## Skill Location & Activation

- **Path:** `skills/create-test-env/SKILL.md`
- **Trust level:** Trusted (workspace `skills/`)
- **Activation keywords:** `create test env`, `spin up instance`, `new ironclaw environment`, `isolated test`, `multi instance`, `test environment`, `throwaway env`
- **Activation patterns:** `"create.*test.*env"`, `"spin.*up.*instance"`, `"new.*ironclaw.*environment"`, `"isolated.*test"`, `"multi.*instance"`, `"throwable.*env"`
- **Exclude keywords:** `docker test`, `production deploy`, `cloud`

## Execution Flow

```
User: "create test env xxx"
  1. Parse environment name from user input
  2. Check ~/.ironclaw-{name} doesn't already exist (prompt if it does)
  3. Scan existing environments for used ports
  4. Read LLM config from main environment (~/.ironclaw/.env)
  5. Create directory: mkdir -p ~/.ironclaw-{name}
  6. Write .env with isolated config
  7. Create PostgreSQL database: createdb ironclaw_{name}
  8. Verify and print startup instructions
```

## .env File Generation

### Inherited from Main Environment

Read `~/.ironclaw/.env` and copy these variables (if present):

- `LLM_BACKEND`
- `NEARAI_API_KEY`, `NEARAI_SESSION_TOKEN`, `NEARAI_BASE_URL`, `NEARAI_MODEL`
- `OPENAI_API_KEY`, `OPENAI_MODEL`
- `ANTHROPIC_API_KEY`, `ANTHROPIC_MODEL`, `ANTHROPIC_BASE_URL`
- `OLLAMA_BASE_URL`, `OLLAMA_MODEL`
- `LLM_BASE_URL`, `LLM_MODEL`, `LLM_API_KEY`
- Any other `LLM_*` or provider-specific vars found

### Newly Generated

| Variable | Value | Rationale |
|----------|-------|-----------|
| `DATABASE_BACKEND` | `"postgres"` | Developer chose PostgreSQL |
| `DATABASE_URL` | `postgres://{user}@{host}:{port}/ironclaw_{name}` | Isolated database per env |
| `DATABASE_POOL_SIZE` | `"5"` | Lower than default 10 for test envs |
| `GATEWAY_PORT` | auto-assigned | Avoid collision |
| `GATEWAY_HOST` | `"127.0.0.1"` | Localhost only |
| `HTTP_PORT` | auto-assigned | Avoid collision |
| `HTTP_HOST` | `"127.0.0.1"` | Localhost only |
| `ONBOARD_COMPLETED` | `"true"` | Skip onboarding wizard |
| `HEARTBEAT_ENABLED` | `"false"` | No background noise in test envs |
| `SANDBOX_ENABLED` | `"false"` | Skip Docker dependency |

### PostgreSQL Connection Defaults

- Host: `localhost` (or inherit from main `DATABASE_URL`)
- Port: `5432` (or inherit from main `DATABASE_URL`)
- User: current OS user (or inherit from main `DATABASE_URL`)
- Password: inherit from main `DATABASE_URL` if present

## Port Allocation

Strategy: scan all `~/.ironclaw-*/.env` files for existing `GATEWAY_PORT` and `HTTP_PORT` values, then pick the lowest unused pair.

Algorithm:
1. Collect all existing gateway ports into a set
2. Collect all existing HTTP ports into a set
3. Starting from 3000 (gateway) / 8080 (HTTP), increment both until finding a pair where neither is used
4. Optionally verify with a quick TCP bind attempt (catch `EADDRINUSE`)

## Database Creation

Two-step process:

1. **Create database:** `createdb ironclaw_{name}`
2. **Enable pgvector extension:** `psql ironclaw_{name} -c "CREATE EXTENSION IF NOT EXISTS vector;"`

pgvector is required for IronClaw's workspace memory hybrid search (vector + FTS). Without it, embedding operations will fail at runtime.

Fallback on failure (permission denied, pg not running):
1. Print the equivalent SQL commands:
   ```sql
   CREATE DATABASE ironclaw_{name};
   \c ironclaw_{name}
   CREATE EXTENSION IF NOT EXISTS vector;
   ```
2. Print psql commands the user can run:
   ```bash
   createdb ironclaw_{name}
   psql ironclaw_{name} -c "CREATE EXTENSION IF NOT EXISTS vector;"
   ```
3. Continue writing the .env — the env is usable once the DB exists and pgvector is enabled

Database name sanitization: lowercase the environment name, replace non-alphanumeric chars with underscores.

## Verification

After creation:
1. Confirm `~/.ironclaw-{name}/.env` exists and contains expected vars
2. Run `psql -l` and confirm `ironclaw_{name}` appears in output
3. Run `psql ironclaw_{name} -c "SELECT extname FROM pg_extension WHERE extname='vector';"` and confirm pgvector is enabled
4. Print summary:
   - Base directory: `~/.ironclaw-{name}`
   - Database: `ironclaw_{name}`
   - Gateway URL: `http://127.0.0.1:{port}/`
   - Start command: `IRONCLAW_BASE_DIR=~/.ironclaw-{name} ironclaw run`

## Error Handling

| Scenario | Behavior |
|----------|----------|
| `~/.ironclaw-{name}` already exists | Ask user: overwrite or choose different name |
| `createdb` fails | Print manual SQL and psql command; env is still usable once DB is created |
| pgvector extension creation fails | Print warning that vector search will not work; suggest `psql <db> -c "CREATE EXTENSION IF NOT EXISTS vector;"` |
| Main environment has no LLM config | Print warning listing which vars to add manually |
| Port occupied by non-IronClaw process | Try TCP bind check; skip to next offset on failure |
| `psql` / `createdb` not in PATH | Print warning with install hint; print manual SQL |

## Out of Scope

- Running / stopping the instance (user runs `ironclaw run` manually)
- Environment teardown / cleanup (user does `rm -rf` + `dropdb`)
- Docker-based environments (covered by `local-test` skill)
- Production or multi-machine deployment
- libSQL backend (PostgreSQL only for test envs)
- Config file generation (TOML) — .env is sufficient
