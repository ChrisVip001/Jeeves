# 发布 Jeeves

English | [RELEASING.md](RELEASING.md)

各渠道，按依赖顺序。**CI 负责构建，人负责发布。** tag 自动触发的只有产物
构建（`.github/workflows/release.yml`）：四个平台的二进制、
以及承载它们的 GitHub Release。npm 的上传在有凭据的机器上执行——
token 不进 CI。

## 0. 前置条件

- [ ] main 分支 `just check` 全绿（fmt / clippy / 测试 / 协议漂移 / ts / py / eval）
- [ ] 版本号在所有落点都已同步（见 §1）
- [ ] 你是有凭据的维护者：npm 账号（`NPM_TOKEN` 或
      `npm login`）、一个能推的 `homebrew-tap` 仓库

## 1. 升版本号

版本号落在四处——必须一致：

| 落点 | 文件 |
|---|---|
| Homebrew formula | `packaging/homebrew/jeeves.rb`（url tag + sha256 —— §3） |
| npm 垫片 | `packaging/npm/package.json` |
| Cargo | `jeeves-rs/jeeves-cli/Cargo.toml`（决定 `jeeves --version`） |

`CHANGELOG.md` + `CHANGELOG.zh.md`：顶部新段落，写真人看的说明（改了什么、
坏了什么、要重配什么）。

## 2. 打 tag、构建

```sh
git tag v0.8.0 && git push origin v0.8.0
# CI 现在构建四个二进制 + 轮子 + GitHub Release。
# 盯进度：gh run watch
```

跑绿之后，GitHub Release 页面持有：

```
jeeves-v0.8.0-aarch64-apple-darwin.tar.gz
jeeves-v0.8.0-x86_64-apple-darwin.tar.gz
jeeves-v0.8.0-x86_64-unknown-linux-gnu.tar.gz
jeeves-v0.8.0-aarch64-unknown-linux-gnu.tar.gz
```

## 3. Homebrew

```sh
just release-hash v0.8.0     # 重写 packaging/homebrew/jeeves.rb 里的 url + sha256
git add packaging/homebrew/jeeves.rb && git commit -m "chore: brew formula v0.8.0" && git push
```

然后更新你的 tap（一个叫 `homebrew-tap` 的仓库）：

```sh
cp packaging/homebrew/jeeves.rb <tap仓库>/Formula/jeeves.rb
cd <tap仓库> && git commit -am "jeeves 0.8.0" && git push
# 在干净机器上验证：
brew tap ChrisVip001/tap && brew install jeeves && brew test jeeves
```

## 5. npm

```sh
cd packaging/npm
npm pack                 # 先检查 tarball：install.js + bin/jeeves.js + package.json + README.md
npm publish              # 每个用户机器上都会跑 install.js —— 一定要先本地试装
```

在临时目录验证：`npm i -g jeeves@0.8.0 && jeeves --version`。

## 6. 发布之后

- [ ] README 的安装说明与现实一致
- [ ] 若 `models-catalog.json` 格式 / 环境变量有变，开一个 release issue 列清楚
- [ ] 下个版本的 CHANGELOG 段落开始留空
