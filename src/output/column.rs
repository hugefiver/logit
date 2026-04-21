use colored::Colorize;

use crate::cli;

pub const COL_SEP: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayCol {
    Commits,
    Adds,
    Dels,
    Changes,
    Net,
    Files,
}

impl DisplayCol {
    pub fn header(&self) -> &'static str {
        match self {
            Self::Commits => "Commits",
            Self::Adds => "Additions",
            Self::Dels => "Deletions",
            Self::Changes => "Changes",
            Self::Net => "Net",
            Self::Files => "Files",
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RowMetric {
    pub commits: u64,
    pub adds: u64,
    pub dels: u64,
    pub files: u64,
}

impl RowMetric {
    pub fn net(&self) -> i64 {
        self.adds as i64 - self.dels as i64
    }
}

pub fn build_display_cols(cols: &[crate::cli::Column], compact: bool) -> Vec<DisplayCol> {
    let mut out = Vec::with_capacity(cols.len());
    let mut i = 0;
    while i < cols.len() {
        match cols[i] {
            cli::Column::Commits => out.push(DisplayCol::Commits),
            cli::Column::Adds => {
                if compact && i + 1 < cols.len() && cols[i + 1] == cli::Column::Dels {
                    out.push(DisplayCol::Changes);
                    i += 2;
                    continue;
                }
                out.push(DisplayCol::Adds);
            }
            cli::Column::Dels => out.push(DisplayCol::Dels),
            cli::Column::Net => out.push(DisplayCol::Net),
            cli::Column::Files => out.push(DisplayCol::Files),
        }
        i += 1;
    }
    out
}

fn net_display(net: i64, num_fmt: cli::NumberFormat) -> String {
    let abs = crate::output::table::format_num(net.unsigned_abs(), num_fmt);
    if net >= 0 {
        format!("+{abs}")
    } else {
        format!("-{abs}")
    }
}

#[derive(Debug, Clone)]
pub struct ColLayout {
    pub cols: Vec<DisplayCol>,
    pub widths: Vec<usize>,
    pub change_add_w: usize,
    pub change_del_w: usize,
}

impl ColLayout {
    pub fn build(
        cols: &[crate::cli::Column],
        compact: bool,
        rows: &[RowMetric],
        num_fmt: cli::NumberFormat,
    ) -> Self {
        let cols = build_display_cols(cols, compact);
        let change_add_w = rows
            .iter()
            .map(|r| format!("+{}", crate::output::table::format_num(r.adds, num_fmt)).len())
            .max()
            .unwrap_or(2);
        let change_del_w = rows
            .iter()
            .map(|r| format!("-{}", crate::output::table::format_num(r.dels, num_fmt)).len())
            .max()
            .unwrap_or(2);

        let widths = cols
            .iter()
            .map(|dc| {
                let data_w = rows
                    .iter()
                    .map(|r| match dc {
                        DisplayCol::Commits => {
                            crate::output::table::format_num(r.commits, num_fmt).len()
                        }
                        DisplayCol::Adds => crate::output::table::format_num(r.adds, num_fmt).len(),
                        DisplayCol::Dels => crate::output::table::format_num(r.dels, num_fmt).len(),
                        DisplayCol::Changes => change_add_w + 1 + change_del_w,
                        DisplayCol::Net => net_display(r.net(), num_fmt).len(),
                        DisplayCol::Files => {
                            crate::output::table::format_num(r.files, num_fmt).len()
                        }
                    })
                    .max()
                    .unwrap_or(1);
                dc.header().len().max(data_w)
            })
            .collect();

        Self {
            cols,
            widths,
            change_add_w,
            change_del_w,
        }
    }
}

pub fn format_cell(
    dc: DisplayCol,
    metric: &RowMetric,
    num_fmt: cli::NumberFormat,
    width: usize,
    change_add_w: usize,
    change_del_w: usize,
    bold: bool,
) -> String {
    match dc {
        DisplayCol::Changes => {
            let add_s = format!(
                "+{}",
                crate::output::table::format_num(metric.adds, num_fmt)
            );
            let del_s = format!(
                "-{}",
                crate::output::table::format_num(metric.dels, num_fmt)
            );
            let add_aligned = format!("{:>w$}", add_s, w = change_add_w);
            let del_aligned = format!("{:>w$}", del_s, w = change_del_w);
            let plain_len = change_add_w + 1 + change_del_w;
            let pad = width.saturating_sub(plain_len);
            if bold {
                format!(
                    "{}{} {}",
                    " ".repeat(pad),
                    add_aligned.green().bold(),
                    del_aligned.red().bold(),
                )
            } else {
                format!(
                    "{}{} {}",
                    " ".repeat(pad),
                    add_aligned.green(),
                    del_aligned.red(),
                )
            }
        }
        DisplayCol::Commits => {
            let s = format!(
                "{:>w$}",
                crate::output::table::format_num(metric.commits, num_fmt),
                w = width,
            );
            if bold {
                s.bright_cyan().bold().to_string()
            } else {
                s.bright_cyan().to_string()
            }
        }
        DisplayCol::Adds => {
            let s = format!(
                "{:>w$}",
                crate::output::table::format_num(metric.adds, num_fmt),
                w = width,
            );
            if bold {
                s.green().bold().to_string()
            } else {
                s.green().to_string()
            }
        }
        DisplayCol::Dels => {
            let s = format!(
                "{:>w$}",
                crate::output::table::format_num(metric.dels, num_fmt),
                w = width,
            );
            if bold {
                s.red().bold().to_string()
            } else {
                s.red().to_string()
            }
        }
        DisplayCol::Net => {
            let net = metric.net();
            let s = format!("{:>w$}", net_display(net, num_fmt), w = width);
            if net >= 0 {
                if bold {
                    s.green().bold().to_string()
                } else {
                    s.green().to_string()
                }
            } else if bold {
                s.red().bold().to_string()
            } else {
                s.red().to_string()
            }
        }
        DisplayCol::Files => {
            let s = format!(
                "{:>w$}",
                crate::output::table::format_num(metric.files, num_fmt),
                w = width,
            );
            if bold {
                s.yellow().bold().to_string()
            } else {
                s.yellow().to_string()
            }
        }
    }
}

pub fn header_row(label_col_header: &str, label_col_w: usize, layout: &ColLayout) -> String {
    let mut row = format!(" {:<w$}", label_col_header.bold(), w = label_col_w);
    for (dc, width) in layout.cols.iter().zip(&layout.widths) {
        row.push_str(&" ".repeat(COL_SEP));
        row.push_str(&format!("{:>w$}", dc.header().bold(), w = width));
    }
    row
}

pub fn data_row(
    label: &str,
    label_w: usize,
    metric: &RowMetric,
    layout: &ColLayout,
    num_fmt: cli::NumberFormat,
    indent: &str,
    bold: bool,
) -> String {
    let label_text = format!("{indent}{label}");
    let mut row = if bold {
        format!(" {:<w$}", label_text.cyan().bold(), w = label_w)
    } else {
        format!(" {:<w$}", label_text.cyan(), w = label_w)
    };

    for (dc, width) in layout.cols.iter().zip(&layout.widths) {
        row.push_str(&" ".repeat(COL_SEP));
        row.push_str(&format_cell(
            *dc,
            metric,
            num_fmt,
            *width,
            layout.change_add_w,
            layout.change_del_w,
            bold,
        ));
    }
    row
}
