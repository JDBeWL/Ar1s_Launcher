# 注释精简手册

一轮注释清理 = 判据 + 范围 + 批量改写 + 证明只动了注释 + 闸门。本文按这五步写，可直接照着复跑。

## 1. 判据

一条一票：**删掉它，谁会写出 bug？** 答不出就删。

删这些：

- 设计迭代史、试错记录（「第一版…后果是…现在改成…」「实测那个做法是错的」）
- 「以前在 X，现在在 Y」式对照（`不再单开凭据文件`、`不再散落在 .download_state/`）
- 调试现场（像素采样值、对比度实测、截图描述）
- 复述下一行代码在做什么
- 对未来改动者的叮嘱（「别在这里另减侧栏」「凡…都该读它」）
- 测试自辩（「做成纯函数以便单测」「这条钉住边界」）与复述 assert 的行
- 同一句话写 N 遍：只在最贴近它的那一处留一次，删调用点那份
- 配置文件里的解释（即使内容是硬约束来源）
- 修辞化收尾（「迟早分岔」「必然漂移」「最难查的一类错」）

留这些，压到 1–2 行：

- 库 / OS / 编译器的反直觉行为（`FILE_SHARE_DELETE` 让直接删锁文件变成误判、Java 版本信息走 stderr、`osx` 而非 `macos`）
- 硬性时序与初始化约束（解锁开关必须先于实验性选项；运行锁要在 flush 之前释放）
- 会误导读者的不变量（`Xms == Xmx` 是刻意的；空分支 `GcStrategy::Default => {}` 为什么是空的）
- 魔数的出处（取样 32 个 class 就够、4 MiB 缓冲装得下一次启动、class 48 = Java 4）
- 隐藏依赖（`{MOJMAPS}` 输出必须落在 `.txt`，否则下一个 processor 指向不存在的文件）
- 日志原文、真实事故的时间戳（`取自本机 2026-10-17 18:40 用 Java 26 启动 … 的真实 stderr`）

模块头保留并推广，别当「文件清单」删：

```rust
//! <模块名，名词短语>
//!
//! <它负责什么，1 行，最多 2 行>
```

前端同形：`/** 模块名 / 一行职责 */`，放在所有 import 之前。要删的是子模块清单和 `1. 2. 3.` 步骤列表，不是「名字 + 职责」本身。

**别答应百分比目标。** 先按类别数行数，够不够；不够就把算术摆出来，别硬删约束。

## 2. 范围

两种口径，先说清用哪种：

- **跟着基点**：`git diff --name-only <上个清理基点>` + 未跟踪文件。适合「我刚改了一批」。
- **全仓**：`git ls-files`。适合季度性梳理；上一轮的清理结果本身也要重读，所以全仓口径才是唯一不漏的口径。

排除：翻译数据（`src/i18n/locales/`）、二进制（icons、`*.icns`——行级分类器会把它们里的 `*` 行当注释报出来）、`dist`、`node_modules`、生成物。

**范围必须显式包含配置与脚本**，不能只扫 `src/`：`Cargo.toml`、`.github/workflows/*.yml`、`scripts/*.mjs`、`vite.config.ts` 里的注释同样在长，而且「配置文件默认不加注释」这条只在它们身上成立。扩展名白名单要带上 `mjs|cjs|toml|ya?ml`。

分组写完要和 `git ls-files` 做差集核对，防止整目录漏在外面（口头写 `components/**`，落到清单里变成逐个文件名，就会漏掉一个目录）。

## 3. 批量改写

单位是**逐条读**，不是砍到某个数。先 dump 全部存活注释：

```
guard.js dump  '^src-tauri/'   # 输出连续注释块 + 起始行 + 块长
guard.js stats                 # 每文件注释行数 / 非空行数 / 密度
guard.js newcm                 # 与 git diff <基点> -U0 取交集，定出本轮新增
```

改动用坐标批处理，别用逐个 Edit（坐标命中即证明读对了行，不命中就跳过，不会误伤）：

```js
// ops.js：{f: 文件, l: 起始行(1-based), n: 覆盖行数, e: 首行去空白后的前缀, to: 替换后的行}
{ f: "src-tauri/src/utils/file_lock.rs", l: 1, n: 9, e: "//! 跨进程文件锁", to: ["//! 跨进程文件锁", "//!", "//! …"] },
{ f: "src-tauri/src/services/instance.rs", l: 1541, n: 1, e: "// 别的锁不受影响", to: [] }, // 纯删
```

`patch.js` 的三重校验，任一不过就 `MISS` 并跳过该 op：首行前缀必须命中；`n` 行必须全是注释（**空行放行**，这样能顺手吃掉删注释后多出来的那一行）；块后一行不能是注释开头（防块长度算错、把代码吞进注释）。每文件按行号降序应用，因此同一文件内多个 op 互不干扰。

`#` 注释只有 TOML / YAML 有，分类与校验都要按扩展名开关；markdown 的 `#` 是标题，必须继续算「代码」，否则改标题没人报警。

### guard.js（剥注释取代码行）

**别手写字符状态机。** 两次的失败方式都是把注释误判成代码然后死循环或崩行号；行级分类器简单且误判方向安全——只会「把注释当代码」，于是删注释时报假警，绝不会漏报代码被改坏。

```js
// 连续行首是注释符号就算注释；行尾注释切掉后半段。
function classify(f) {
  const out = [];
  let inBlock = false;
  for (const raw of read(f)) {
    const t = raw.trim();
    if (!t) continue;
    if (inBlock) {
      if (t.includes('*/') || t.includes('-->')) inBlock = false;
      continue;
    }
    if (/^\/\*/.test(t)) {
      if (!t.includes('*/')) {
        inBlock = true;
        continue;
      }
      const tail = t.slice(t.indexOf('*/') + 2).trim();
      if (tail) out.push(tail);
      continue;
    }
    if (/^\/\//.test(t) || /^<!--/.test(t) || /^\*\s/.test(t) || /^\*[^/]?$/.test(t)) continue;
    if (/^-->/.test(t)) continue;
    let code = t;
    const idx = findLineComment(code);
    if (idx >= 0) code = code.slice(0, idx).trim();
    if (code) out.push(code);
  }
  return out;
}
```

模式：`baseline` 把每文件的代码行数组快照存 `baseline.json`；`check` 重算并逐文件比对，报出 `lost/new` 行，非零退出。

### patch.js（坐标改写）

```js
const OPS = require('./' + process.argv.find((a) => a.startsWith('--ops=')).slice(6));
const isComment = (t) => /^(\/\/|\/\*|\*)/.test(t) || /^<!--/.test(t);
const isCommentOpen = (t) => /^(\/\/|\/\*|\*[^/])/.test(t) || /^\*+$/.test(t) || /^<!--/.test(t);

for (const [f, ops] of byFile) {
  const rawText = fs.readFileSync(path.join(ROOT, f), 'utf8');
  const eol = rawText.includes('\r\n') ? '\r\n' : '\n'; // 仓库里混着 CRLF 文件，join 必须按原 EOL
  const lines = rawText.split(/\r?\n/);
  ops.sort((a, b) => b.l - a.l);
  for (const op of ops) {
    const first = (lines[op.l - 1] || '').trim();
    if (!first.startsWith(op.e)) {
      miss(f, op.l);
      continue;
    }
    for (let i = 0; i < op.n; i++) if (!isComment((lines[op.l - 1 + i] || '').trim())) miss(f, op.l + i);
    if (isCommentOpen((lines[op.l - 1 + op.n] || '').trim())) miss(f, op.l);
    const indent = (lines[op.l - 1].match(/^\s*/) || [''])[0];
    lines.splice(op.l - 1, op.n, ...op.to.map((s) => (s === '' ? '' : indent + s)));
  }
  if (write) fs.writeFileSync(path.join(ROOT, f), lines.join(eol));
}
```

`--check` 先跑（`misses=0` 才允许 `--write`），`--write` 后立刻 `guard.js check`。

## 4. 实测踩过的坑

- **脚本别只放 `%TEMP%`。** 第四轮开工时 `comment_round\` 已被系统清空，靠本文内嵌的代码重建才继续下去；要长期用就把它放进仓库（如 `scripts/`）。
- **新功能带来的一大类冗余是「以前…现在…」**：一批特性落地（分段下载、竞速、指纹缓存、导出整合包）就 +654 行注释、密度 +1.15pt，其中反复出现的句式是「以前这一步混在 X 里」「以前用 `let _ =` 吞错」「三处以前各自写一遍」。这类只留结论那一行，改法本身不是注释的内容。
- **`to` 里代表「doc 注释中间那个空行」必须写成 `///` / `//!` / ` *`，不能写 `""`。** 裸空行会把 Rust doc 注释劈成两段，`cargo` 和 `cargo fmt` 都不报，只有肉眼看 diff 才发现。收尾可以跑一遍「找夹在两条 `///` 之间的空行」的脚本来兜住。
- **删注释会改动 rustfmt 的判据**：`}` 与 `else if` 之间原本隔着注释，删掉后 rustfmt 要求合成 `} else if`。这是唯一会被 `fmt --check` 抓的非注释改动，要单独向用户点名。
- **看着像复述、其实不能删**：空分支的解释、哨兵值、`// 不加参数，行为等同官方启动器` 这类「为什么没写代码」。
- **并行 agent 会真删代码**：一轮 11 组里出过 4 次事故（整块 zip 路径遍历守卫被删、注释行被换成上一行 `let` 的副本、`use` 被吞），而这些组自称「零代码改动」。别信回报，收尾一律逐文件剥离对比 + `git diff --numstat` 核。
- **agent 改写注释会引入 lint**：把普通反引号升级成 `[`X`]` intra-doc 链接（指向 `pub(super)` 就出 rustdoc 警告 → 必须 `cargo doc --no-deps`）；写没包反引号的文件名清单触发 `clippy::doc_markdown` → 必须跑 clippy。
- **代码注释里会残留 md 加粗 `**…**`**：写的时候以为是文档，读的时候只看到成对星号，强调全丢了。扫 `grep -rnP '(?<!/)\*\*[^*/]{1,80}\*\*(?!/)'`，命中了直接把星号删掉、文字留下；`/**` 开头、`src/**/*` 这类 glob、JS 幂运算符 `a ** b`、`--accessToken=***` 掩码都是合法星号，别一起改。
- **整理注释会顺带挖出事实性错误**：doc 标错对象（时长常量被标成「悬停底色」）、README 与代码脱节（写死的列数/尺寸早被重构推翻）。文档里每条数字和行为都要回代码验，别照抄旧稿。
- 改完不要只看 diff：把**存活注释全 dump 出来通读一遍**，漏改的目录和自相矛盾的表述会自己浮出来。

## 5. 闸门（本仓库）

```bash
node guard.js check                                   # 代码行完全一致，否则前功尽弃
cd src-tauri && cargo fmt --check
cd src-tauri && cargo test --lib                      # 387 passed（2026-10-22）
cd src-tauri && cargo clippy --all-targets -- -D warnings
pnpm exec vue-tsc --noEmit                            # 唯一会检查 .vue 的闸门
pnpm exec vitest run                                  # 162 passed / 17 files（含 scripts/*.test.mjs）
pnpm exec prettier --check .                          # .prettierignore 排掉 src-tauri/dist/public
pnpm version:check                                    # 改到 Cargo.toml / tauri.conf.json / README 时要跑
```
