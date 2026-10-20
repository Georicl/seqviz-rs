<p align="center">
  <img src="docs/assets/logo.png" width="200" alt="seqviz-rs logo">
</p>

# seqviz-rs

[English](README.md) | [简体中文](README.zh-CN.md)

**在终端里看清序列和测序质量。**

`sq` 是一个生物序列浏览器。给它一个文件路径，就能在左右双栏中查看记录和序列；多个文件会打开成标签页。支持核酸与蛋白质 FASTA、FASTQ，以及 SAM/BAM，在 macOS 和 Linux 上使用，界面为英文。

```sh
sq genome.fa
sq reads.fastq.gz
sq proteins.faa alignments.bam
```

在 yazi 中也可以按 Enter 打开，浏览完按 `q` 回到文件列表。

[![终端演示：FASTA 选区与复制、gzip FASTQ 质量显示、蛋白质序列、BAM 属性和统计页](docs/assets/demo.gif)](docs/assets/demo.gif)

## 能看到什么

| 文件 | 显示内容 |
| --- | --- |
| 核酸 FASTA | 碱基着色，完整 header、长度和 GC |
| 蛋白质 FASTA | 氨基酸分组着色，完整 header 和序列长度 |
| FASTQ | 按质量着色的碱基，逐字符质量副行、质量概览条和 Q30 等属性 |
| SAM / BAM | 每条比对记录的序列与质量，以及参考序列、位置、FLAG、MAPQ、CIGAR 和附加字段 |

支持 gzip 和 BGZF 压缩输入。FASTQ 接受跨行序列与质量，使用 Phred+33，并解析 Illumina、Nanopore、PacBio 和 SRA 的 header；原始 header 始终保留。

序列随窗口宽度自动换行，位置从 1 开始。切换标签时，各文件保留自己的当前记录和滚动位置。FASTA/FASTQ 可以用鼠标跨行选择序列，拖到边缘继续滚动；缩放窗口后选区仍对应原来的序列区间。

## 安装与试用

需要 Rust、Cargo 和 C 编译器。在源码目录安装 `sq`。

```sh
git clone https://github.com/Georicl/seqviz-rs.git
cd seqviz-rs
cargo install --path . --locked
sq --version
sq test/test.fa
```

也可以先运行仓库里的示例，不安装命令。

```sh
cargo run --release --locked -- test/test.fa test/alignments.bam
```

传入多个路径，就能在同一个浏览器中切换文件。文件名以 `-` 开头时，在路径前加 `--`。

```sh
sq assembly.fa reads.fastq.gz proteins.faa
sq -- "-sample.fa"
```

## 常用操作

| 操作 | 按键或鼠标 |
| --- | --- |
| 上下移动记录或序列光标 | ↑ / ↓，或 `j` / `k` |
| 左右移动序列光标 | ← / →，或 `h` / `l` |
| 上一条 / 下一条记录 | `[` / `]` |
| 切换左侧列表 / 右侧序列焦点 | 鼠标点击，或 `Ctrl+h` / `Ctrl+l` |
| 翻页，或跳到开头 / 末尾 | Page Up / Page Down，Home / End |
| 按名称部分匹配搜索 | `/` 输入名称后按 Enter；`n` 跳到下一个匹配 |
| 跳到当前序列的指定位置 | `:` 输入 1-based 坐标后按 Enter |
| 下一个 / 上一个文件标签 | Tab / Shift+Tab |
| 关闭当前文件标签 | `x` |
| 选择并复制序列 | 鼠标拖选，或 `v` 后移动光标；`y` 复制 |
| 显示 / 隐藏质量字符副行 | `Q` |
| 帮助 / 文件 / 当前记录 / 统计 | `?` 或 F1 / F2 / F3 / F4 |
| 取消任务并暂停后台索引 | 浏览界面中按 Esc |
| 退出信息页 / 退出浏览器 | `q` |

鼠标滚轮滚动指针所在区域。信息页中可用 Tab 或左右方向键切换页面。完整按键说明在 `?` 帮助页中。

`F4` 的统计页显示记录数、总长度及最短/最长/平均长度，并提供适用格式的 N50。GC 保留两位小数。统计按需在后台计算，尚未完成的扫描会显示状态。SAM/BAM 按每条比对记录计数，不去重，也不显示 N50。

## 从 yazi 打开

安装后，确保 yazi 能从 PATH 找到 `sq`，再将对应示例合并到 `~/.config/yazi/yazi.toml`。

| yazi 版本 | 配置示例 |
| --- | --- |
| 25.4 | [contrib/yazi.toml](contrib/yazi.toml) |
| 26.9 | [contrib/yazi-26.toml](contrib/yazi-26.toml) |

配置中的 opener 使用 `block = true`。选中文件按 Enter，`sq` 就会接管当前终端；退出后返回 yazi 原来的界面。

## 复制序列

FASTA/FASTQ 中，`y` 根据当前焦点复制内容。

| 位置 | 复制结果 |
| --- | --- |
| FASTA 左侧记录列表 | 完整 header 和完整序列，每行 60 个字符 |
| FASTQ 左侧记录列表 | 完整 FASTQ 记录，保留原始 header 和分隔行 |
| 右侧序列选区 | 连续序列，不包含显示换行、位置编号和质量副行 |
| 信息页 | 所选文字；没有选区时复制当前页 |

**SAM/BAM 禁用全部复制功能，包括信息页。**

本地优先使用系统剪贴板：macOS 的 `pbcopy`，Linux 的 `wl-copy`、`xclip` 或 `xsel`。SSH 下通过 OSC 52 请求本地终端写入剪贴板，需要终端支持。复制完整记录会读取并暂存整条记录的内容。

## 大文件与缓存

`sq` 先读取首屏，再逐块建立索引。记录列表和序列内容按当前窗口加载，浏览时不把整个文件装入内存。索引与压缩检查点会保存下来，再次打开可继续使用；源文件变化后会使旧索引失效。首次访问尚未索引的远处位置，仍需推进扫描或解压。

缓存位于 `$XDG_CACHE_HOME/seqviz-rs/`，默认是 `~/.cache/seqviz-rs/`。缓存退出后仍保留，不会自动清理；关闭所有 `sq` 后可手动删除，再次打开时会重建。普通 gzip 使用解压检查点，不生成完整解压副本。

原始文件保持只读。将 FASTA 输出重定向到文件或管道时，`sq` 会输出文本；其他格式需要交互终端。

## 配色

默认主题开箱可用。需要调整颜色时，将 [config/config.toml](config/config.toml) 和 [config/content.toml](config/content.toml) 放到 `~/.config/seqviz-rs/`；设置了绝对路径的 `XDG_CONFIG_HOME` 时，使用其下的 `seqviz-rs/` 目录。

`config.toml` 配置背景、边框等界面颜色，并引用 `content.toml` 中的碱基、氨基酸和质量配色。
