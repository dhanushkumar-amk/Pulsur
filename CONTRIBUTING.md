# Contributing to pulsar 🛸

Thank you for your interest in contributing! This project is built on **Rust** and **TypeScript**, and we value code quality and clarity.

## 📝 Commit Conventions

We strictly follow [Conventional Commits](https://www.conventionalcommits.org/). Any contribution must use a descriptive prefix to help with automated changelogs.

| Prefix | Description |
| :--- | :--- |
| `feat:` | A new feature for the user |
| `fix:` | A bug fix for the user |
| `chore:` | Internal maintenance (no production code changes) |
| `docs:` | Documentation update |
| `test:` | Adding or improving tests |
| `perf:` | Performance improvements |
| `refactor:` | Code restructuring without change in behavior |

**Examples:**
- `feat(gateway): add request routing logic`
- `fix(queue): resolve race condition in job polling`
- `chore: update workspace dependencies`

## 🔨 Development Workflow

1.  **Fork the repo** and create your branch from `main`.
2.  **Run tests** before submitting:
    ```bash
    cargo test --workspace
    npm test --workspaces
    ```
3.  **Run formatting**:
    ```bash
    cargo fmt --all
    npm run lint
    ```
4.  **Open a Pull Request** with a detailed description of your changes.

## ⚖️ Code of Conduct

Maintainers and contributors are expected to treat everyone with respect and follow the project's code of conduct.

## 🚀 CI/CD Expectations

Every PR will trigger a CI pipeline that runs:
- Rust Linting (`clippy`)
- JS/TS Linting (`eslint`)
- Unit/Integration Tests
- Build verification

PRs will not be merged unless all checks pass.

