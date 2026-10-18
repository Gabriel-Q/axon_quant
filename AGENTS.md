# axon_quant 提交铁律

CI 用 `cargo xxx --locked`（validation.yml），**Cargo.lock 必须与 Cargo.toml 完全一致且入库**。
之前因版本号改了但 lock 未 regenerate 导致 clippy 挂掉（exit code 101）。

## 版本号变更流程（必走）

版本号在 `Cargo.toml`（workspace.package.version）和 `pyproject.toml` 两处声明，
各 crate 的 Cargo.toml 用 `version.workspace = true` 继承，**不要单独改 crate 版本**。

```bash
# 1. 改两处版本号（Cargo.toml + pyproject.toml）
# 2. regenerate lock 文件（不跑网络，只用本地 Cargo.toml）
cargo generate-lockfile
# 3. 跑 CI 等价校验
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test  --workspace --locked
cargo build --workspace --locked
# 4. 确认 Cargo.lock 有变动并入库
git diff -- Cargo.lock   # 应看到各 crate 版本号同步变更
git add Cargo.toml pyproject.toml Cargo.lock
# 5. 切分支（分支名 = 版本号，如 0.17.1）
git checkout -b 0.17.1
```

## commit 前本地校验（不能省）

```bash
cargo fmt --all                              # pre-commit 自动跑
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy --workspace --all-targets --features python --locked -- -D warnings
                                             # = make python-lint，见下
cargo test  --workspace --locked             # 跑过再提交,避免 CI 才炸
```

**铁律**：CI 挂了再修的成本远高于本地跑一遍。没有 `cargo test --locked`
跑过的改动不许提交。

**python feature 例外**：绑定层（`crates/*/src/python`）由 `#[cfg(feature = "python")]`
门控，上面这条不带 feature 的 clippy 编译不到它们，改动绑定层时必须补跑
`make python-lint`（CI 已有同名 job，是阻断性的）。注意绑定层的单测**只能编译校验、
无法运行**：pyo3 的 `extension-module` 不链接 libpython，`cargo test --features python`
必然在链接阶段失败。详见 `docs/zh/about/contributing.md`。

## pre-commit hook（可选）

仓库根已存 `.pre-commit-config.yaml`，安装方式：

```bash
# 一次性安装 pre-commit
pip install pre-commit      # 或 uv tool install pre-commit
cd axon_quant && pre-commit install
```

之后 git commit 自动跑 `cargo fmt --all` + `cargo clippy --locked`。
clippy 较慢（~3min），可临时 `SKIP=cargo-clippy git commit ...` 跳过，
但最终提交前必须手动跑一遍。
