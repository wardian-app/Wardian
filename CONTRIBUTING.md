# Contributing to Wardian

We would love to accept your patches and contributions! Wardian is a community-driven project, and we value every contribution that helps evolve the agent habitat.

## 🚀 Getting Started

1.  **Find an Issue**: Browse our [GitHub Issues](https://github.com/wardian-app/Wardian/issues). We use labels like `help-wanted` and `complexity-low` for new contributors.
2.  **Fork & Branch**: Fork the repository and create a new branch for your feature or bug fix.
    ```bash
    git checkout -b feat/your-feature-name
    ```
3.  **Development Environment**:
    - **Frontend**: Node.js 20+
    - **Backend**: Rust 1.75+
    - Run `npm install` in the root.
    - Run `npm run dev` to start the Tauri development environment.

## 🛠️ Code Contribution Process

### 1. Unified Architecture
Wardian follows a strict **Modular Domain Design**.
- All backend logic must reside in `src-tauri/src/`.
- All frontend components must follow the hierarchy in `src/`.
- Ensure you respect **State Sovereignty** (Rust is the source of truth).

### 2. Verification
Before submitting a PR, ensure all quality gates pass:
- **Frontend**: `npm run lint` and `npm run test`
- **Backend**: `cargo clippy` and `cargo test`

### 3. Pull Request Guidelines
- **Small & Atomic**: We prefer small PRs that focus on a single fix or feature.
- **Link Issues**: Always link your PR to an existing issue (e.g., `Fixes #123`).
- **Description**: Use our PR template to provide context and testing proof.
- **Specs**: If introducing a significant architectural change, ensure you have first proposed a **Spec** in `docs/specs/`.

## 📚 Documentation
If your PR changes the user experience or adds a new feature, you **must** update the relevant guides in `docs/guide/` or `docs/developer/`.

## 🤝 Community Guidelines
This project follows standard open-source conduct. Be respectful, professional, and collaborative.

We're looking forward to seeing your contributions!
