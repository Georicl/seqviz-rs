<p align="center">
  <img src="assets/logo.png" width="200" alt="seqviz-rs logo">
</p>

# seqviz-rs

[English](README.md) | [简体中文](README.zh-CN.md)

**在终端里查看序列、测序质量和变异。**

`sq` 是一个轻量的生物序列与变异浏览器，支持核酸和蛋白质 FASTA、FASTQ、SAM/BAM，以及 v0.2.0 新增的 VCF。可直接打开 gzip、BGZF 压缩文件，也可以将多种格式放在同一个窗口的不同标签中浏览。适用于 macOS 和 Linux，界面为英文。

```sh
sq genome.fa
sq reads.fastq.gz
sq genome.fa variants.vcf.gz
```

[![v0.2.0 终端演示：序列浏览、VCF 变异表、contig 选择、样本基因型与原始记录](assets/demo.gif)](assets/demo.gif)

## v0.2.0 新增

- **VCF 浏览**：变异主表、可搜索的 contig 选择器、坐标跳转，以及 INFO、Samples、Raw 三种详情。
- **变异与样本配色**：REF/ALT 沿用碱基颜色，GT 按等位基因编号着色，AD 条展示对应等位基因的深度；区分 PASS、过滤失败与缺失值。
- **按上下文复制**：复制当前变异原始行、INFO 字段、样本及字段标题，或带原始 header 的完整记录；支持详情文字拖选。
- **混合文件标签**：在序列与 VCF 之间切换，各文件保留浏览位置。现有配置继续可用，新增可选的 `[vcf]` 设置。

## 支持的文件

| 文件 | 主要内容 |
| --- | --- |
| 核酸 FASTA | 碱基着色、完整 header、长度与 GC |
| 蛋白质 FASTA | 氨基酸分组着色、完整 header 与长度 |
| FASTQ | 按 Phred+33 质量着色、质量字符副行、质量概览及 Q30 等属性 |
| SAM / BAM | 比对记录的序列、质量、参考坐标、FLAG、MAPQ、CIGAR 与附加字段 |
| 普通 VCF 4.0–4.5 | 变异表、contig/坐标导航、INFO、样本字段、原始 header 与记录 |

gzip 和 BGZF 适用于上述输入。FASTQ 接受跨行序列与质量字段，并解析 Illumina、Nanopore、PacBio 和 SRA header；保留原始 header。

## 安装

### Homebrew

支持 macOS Apple Silicon、Intel 和 Linux x86_64。安装后的命令为 `sq`。

```sh
brew install Georicl/tap/seqviz-rs
sq --version
```

升级：

```sh
brew upgrade Georicl/tap/seqviz-rs
```

主题和 yazi 配置示例位于 `$(brew --prefix seqviz-rs)/share/seqviz-rs/` 下的 `config/` 与 `contrib/`。

### 下载二进制

从 [最新 Release](https://github.com/Georicl/seqviz-rs/releases/latest) 下载，无需安装 Rust。

| 平台 | 安装包 |
| --- | --- |
| macOS 11+ · Apple Silicon | [aarch64-apple-darwin](https://github.com/Georicl/seqviz-rs/releases/latest/download/seqviz-rs-aarch64-apple-darwin.tar.gz) |
| macOS 11+ · Intel | [x86_64-apple-darwin](https://github.com/Georicl/seqviz-rs/releases/latest/download/seqviz-rs-x86_64-apple-darwin.tar.gz) |
| Linux · x86_64 | [x86_64-unknown-linux-musl](https://github.com/Georicl/seqviz-rs/releases/latest/download/seqviz-rs-x86_64-unknown-linux-musl.tar.gz) |

解压后，在解压目录执行：

```sh
mkdir -p "$HOME/.local/bin"
install -m 755 sq "$HOME/.local/bin/sq"
export PATH="$HOME/.local/bin:$PATH"
sq --version
```

将 `~/.local/bin` 加入 shell 的 PATH 后，新终端也能使用 `sq`。安装包附带主题与 yazi 配置示例，Release 同时提供 [SHA-256 校验文件](https://github.com/Georicl/seqviz-rs/releases/latest/download/SHA256SUMS)。

### 从源码安装

需要 Rust stable、Cargo 和 C 编译器。

```sh
git clone https://github.com/Georicl/seqviz-rs.git
cd seqviz-rs
cargo install --path . --locked
sq --version
```

也可以直接运行仓库示例：

```sh
cargo run --release --locked -- test/test.fa test/alignments.bam test/variants.vcf
```

## 常用操作

| 全局操作 | 按键 |
| --- | --- |
| 下一个 / 上一个文件 | Tab / Shift+Tab，或点击标签 |
| 关闭当前文件 | `x` |
| 帮助 / 文件信息 | `?` 或 F1 / F2 |
| 退出信息页 / 浏览器 | `q` |

鼠标滚轮滚动指针所在区域。文件名以 `-` 开头时，使用 `sq -- "-sample.fa"`。

### 序列浏览

| 操作 | 按键或鼠标 |
| --- | --- |
| 移动记录或序列光标 | ↑ / ↓ / ← / →，或 `j` / `k` / `h` / `l` |
| 上一条 / 下一条记录 | `[` / `]` |
| 切换记录列表 / 序列焦点 | 点击区域，或 Ctrl+h / Ctrl+l |
| 翻页 / 开头 / 末尾 | Page Up / Page Down / Home / End |
| 按名称搜索 / 下一个匹配 | `/` 输入后按 Enter / `n` |
| 跳到序列坐标 | `:` 输入 1-based 坐标后按 Enter |
| 选择并复制序列 | 鼠标拖选，或 `v` 后移动光标；`y` 复制 |
| 显示 / 隐藏质量字符副行 | `Q` |
| 当前记录 / 统计 | F3 / F4 |
| 取消任务并暂停索引 | 浏览界面中按 Esc |

序列随窗口自动换行。FASTA/FASTQ 可跨行拖选，在边缘停留会继续滚动；缩放后保留选中的序列坐标。F4 显示记录数、总长度、最短/最长/平均长度及适用格式的 N50；SAM/BAM 按比对记录计数。

### VCF 浏览

主表显示 CHROM、POS、ID、REF、ALT、TYPE、QUAL、FILTER；窄窗口优先保留 CHROM、POS、REF、ALT。多 ALT 包含不同变异类型时显示 MIXED。

| 操作 | 按键 |
| --- | --- |
| 移动变异或详情行 | ↑ / ↓ 或 `j` / `k`；Page Up / Page Down |
| 聚焦详情 / 返回主表 | Enter / Esc |
| INFO / Samples / Raw | `1` / `2` / `3`，或 `[` / `]` |
| 横向滚动详情 | ← / →，或 `h` / `l` |
| 选择 contig | `c`，输入名称筛选后按 Enter；也可选择 All records |
| 在当前 contig 定位 | `:` 输入 1-based 坐标后按 Enter |
| 原始 header | `m` 或 F3 |
| 按当前上下文复制 | `y` |
| 在主表或 Raw 中复制 header 与当前记录 | `Y` |

Samples 保留 FORMAT 字段顺序，按需解析可见样本。GT 颜色代表等位基因编号，AD 使用相同顺序；QUAL、GQ 显示原始数值，FILTER `.` 与 PASS 分开显示。

## 复制内容

| 位置 | `y` 的结果 |
| --- | --- |
| FASTA 记录列表 | 完整 header 和完整序列，每行 60 字符 |
| FASTQ 记录列表 | 完整 FASTQ 记录，保留原始 header 与分隔行 |
| 序列选区 | 连续序列，不带显示换行、坐标或质量副行 |
| VCF 主表 / Raw | 当前原始记录行，保留 TAB |
| VCF INFO | 当前键值 |
| VCF Samples | 当前样本及字段标题 |
| 详情文字选区 / 信息页 | 所选文字；信息页无选区时复制整页 |

VCF 主表或 Raw 中的 `Y` 复制原始 header 与当前记录；VCF 的其他位置中，`Y` 沿用 `y` 的上下文规则。**SAM/BAM 禁用全部复制功能，包括信息页。**

本地优先使用 `pbcopy`（macOS）或 `wl-copy`、`xclip`、`xsel`（Linux）。SSH 使用 OSC 52，需要终端支持。复制完整记录时会读取并暂存整条记录。

## 从 yazi 打开

将对应配置合并到 `~/.config/yazi/yazi.toml`，并确保 `sq` 在 PATH 中：

| yazi 版本 | 配置示例 |
| --- | --- |
| 25.4 | [contrib/yazi.toml](contrib/yazi.toml) |
| 26.9 | [contrib/yazi-26.toml](contrib/yazi-26.toml) |

opener 使用 `block = true`。选中文件按 Enter 打开，按 `q` 退出后返回 yazi。

## 大文件与配置

`sq` 先显示首屏，再分块建立索引。可见记录与内容按窗口读取，VCF 只保留当前选中的原始记录并按需解析样本。索引与解压检查点保存在 `$XDG_CACHE_HOME/seqviz-rs/`，默认 `~/.cache/seqviz-rs/`；再次打开可复用，源文件变化会使旧索引失效。普通 gzip 使用解压检查点，不生成完整解压副本。首次访问尚未索引的远处位置仍需扫描或解压。

原始文件保持只读。FASTA 可以重定向输出文本，其他格式使用交互终端。

默认主题开箱可用。将 [config/config.toml](config/config.toml) 与 [config/content.toml](config/content.toml) 放到 `~/.config/seqviz-rs/`；使用绝对路径的 `XDG_CONFIG_HOME` 时，放到其下的 `seqviz-rs/` 目录。

`config.toml` 配置界面颜色及可选的 `[vcf]` 设置：过滤状态、等位基因颜色、AD 条和详情高度。`content.toml` 配置共享碱基、氨基酸和 FASTQ 质量颜色。旧配置无需新增字段即可继续使用。

## 参与开发

v0.2.0 将应用外壳、公共服务、通用组件和格式模块分别组织到 `app`、`core`、`widgets`、`modules`。各格式模块维护自己的读取、数据模型、页面和配色，方便独立开发。

开发入口、检查命令及问题反馈方式见 [贡献指南](CONTRIBUTING.md)。

## 许可协议

[MIT](LICENSE)。
