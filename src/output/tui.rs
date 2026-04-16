use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState},
    DefaultTerminal, Frame,
};

use crate::stats::models::PeriodStats;

#[derive(Clone, Copy, Debug, PartialEq)]
enum ViewMode {
    Tree,
    Flat,
}

fn top_language(period: &PeriodStats) -> String {
    period
        .by_language
        .iter()
        .max_by_key(|(_, s)| s.additions)
        .map(|(name, _)| name.clone())
        .unwrap_or_else(|| "—".to_string())
}

fn sorted_languages(period: &PeriodStats) -> Vec<(&String, u64, u64)> {
    let mut langs: Vec<_> = period
        .by_language
        .iter()
        .map(|(name, ls)| (name, ls.additions, ls.deletions))
        .collect();
    langs.sort_by(|a, b| (b.1 + b.2).cmp(&(a.1 + a.2)));
    langs
}

fn lang_tree_row<'a>(
    prefix: &str,
    lang_name: &str,
    adds: u64,
    dels: u64,
    dim: Style,
    dim_green: Style,
    dim_red: Style,
) -> Row<'a> {
    let net: i64 = adds as i64 - dels as i64;
    let net_str = if net >= 0 {
        format!("+{net}")
    } else {
        format!("{net}")
    };
    Row::new([
        Cell::from(Line::from(Span::styled(
            format!("{prefix}{lang_name}"),
            dim,
        ))),
        Cell::from(""),
        Cell::from(Line::from(vec![
            Span::styled(format!("{net_str} ("), dim),
            Span::styled(format!("+{adds}"), dim_green),
            Span::styled(" ", dim),
            Span::styled(format!("-{dels}"), dim_red),
            Span::styled(")", dim),
        ])),
        Cell::from(""),
        Cell::from(""),
    ])
}

fn lang_flat_row<'a>(
    lang_name: &str,
    adds: u64,
    dels: u64,
    dim: Style,
    dim_green: Style,
    dim_red: Style,
) -> Row<'a> {
    let net: i64 = adds as i64 - dels as i64;
    let net_str = if net >= 0 {
        format!("+{net}")
    } else {
        format!("{net}")
    };
    Row::new([
        Cell::from(Line::from(Span::styled(format!("  {lang_name}"), dim))),
        Cell::from(Line::from(Span::styled(net_str, dim))),
        Cell::from(Line::from(Span::styled(format!("+{adds}"), dim_green))),
        Cell::from(Line::from(Span::styled(format!("-{dels}"), dim_red))),
        Cell::from(""),
    ])
}

pub struct TuiApp {
    stats: Vec<PeriodStats>,
    totals: PeriodStats,
    table_state: TableState,
    should_quit: bool,
    view_mode: ViewMode,
}

impl TuiApp {
    pub fn new(stats: Vec<PeriodStats>, totals: PeriodStats) -> Self {
        let mut table_state = TableState::default();
        if !stats.is_empty() {
            table_state.select(Some(0));
        }
        Self {
            stats,
            totals,
            table_state,
            should_quit: false,
            view_mode: ViewMode::Tree,
        }
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
        while !self.should_quit {
            terminal.draw(|frame| self.render(frame))?;
            self.handle_events()?;
        }
        Ok(())
    }

    fn render(&mut self, frame: &mut Frame) {
        let [header_area, main_area, footer_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());

        self.render_header(frame, header_area);
        self.render_table(frame, main_area);
        self.render_footer(frame, footer_area);
    }

    fn render_header(&self, frame: &mut Frame, area: Rect) {
        let version = env!("CARGO_PKG_VERSION");
        let header = Paragraph::new(Line::from(vec![
            Span::styled(
                "logit — lines of git",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("  v{version}")),
        ]));
        frame.render_widget(header, area);
    }

    fn render_table(&mut self, frame: &mut Frame, area: Rect) {
        let header = Row::new([
            Cell::from("Period"),
            Cell::from("Commits"),
            Cell::from("Additions"),
            Cell::from("Deletions"),
            Cell::from("Top Language"),
        ])
        .style(Style::default().bold())
        .bottom_margin(1);

        let widths = [
            Constraint::Min(16),
            Constraint::Min(10),
            Constraint::Min(12),
            Constraint::Min(12),
            Constraint::Fill(1),
        ];

        let green = Style::default().fg(Color::Green);
        let red = Style::default().fg(Color::Red);
        let dim = Style::default().fg(Color::DarkGray);
        let dim_green = Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::DIM);
        let dim_red = Style::default().fg(Color::Red).add_modifier(Modifier::DIM);

        let mut all_rows: Vec<Row> = Vec::new();

        for p in &self.stats {
            all_rows.push(Row::new([
                Cell::from(p.period_label.clone()),
                Cell::from(p.total_commits.to_string()),
                Cell::from(Line::from(Span::styled(
                    format!("+{}", p.total_additions),
                    green,
                ))),
                Cell::from(Line::from(Span::styled(
                    format!("-{}", p.total_deletions),
                    red,
                ))),
                Cell::from(top_language(p)),
            ]));

            let langs = sorted_languages(p);
            for (i, (lang_name, adds, dels)) in langs.iter().enumerate() {
                all_rows.push(match self.view_mode {
                    ViewMode::Tree => {
                        let prefix = if i == langs.len() - 1 {
                            "└── "
                        } else {
                            "├── "
                        };
                        lang_tree_row(prefix, lang_name, *adds, *dels, dim, dim_green, dim_red)
                    }
                    ViewMode::Flat => {
                        lang_flat_row(lang_name, *adds, *dels, dim, dim_green, dim_red)
                    }
                });
            }
        }

        all_rows.push(
            Row::new([
                Cell::from("───"),
                Cell::from("───"),
                Cell::from("───"),
                Cell::from("───"),
                Cell::from("───"),
            ])
            .style(Style::default().fg(Color::DarkGray)),
        );

        let totals_bold = Style::default().bold();
        all_rows.push(Row::new([
            Cell::from(Line::from(Span::styled("Total", totals_bold))),
            Cell::from(Line::from(Span::styled(
                self.totals.total_commits.to_string(),
                totals_bold,
            ))),
            Cell::from(Line::from(Span::styled(
                format!("+{}", self.totals.total_additions),
                green.add_modifier(Modifier::BOLD),
            ))),
            Cell::from(Line::from(Span::styled(
                format!("-{}", self.totals.total_deletions),
                red.add_modifier(Modifier::BOLD),
            ))),
            Cell::from(Line::from(Span::styled(
                top_language(&self.totals),
                totals_bold,
            ))),
        ]));

        let total_langs = sorted_languages(&self.totals);
        for (i, (lang_name, adds, dels)) in total_langs.iter().enumerate() {
            all_rows.push(match self.view_mode {
                ViewMode::Tree => {
                    let prefix = if i == total_langs.len() - 1 {
                        "└── "
                    } else {
                        "├── "
                    };
                    lang_tree_row(prefix, lang_name, *adds, *dels, dim, dim_green, dim_red)
                }
                ViewMode::Flat => lang_flat_row(lang_name, *adds, *dels, dim, dim_green, dim_red),
            });
        }

        let highlight_style = Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD);

        let title = match self.view_mode {
            ViewMode::Tree => "Stats [tree]",
            ViewMode::Flat => "Stats [flat]",
        };

        let table = Table::new(all_rows, widths)
            .header(header)
            .block(Block::default().borders(Borders::ALL).title(title))
            .row_highlight_style(highlight_style)
            .highlight_symbol(">> ");

        frame.render_stateful_widget(table, area, &mut self.table_state);
    }

    fn render_footer(&self, frame: &mut Frame, area: Rect) {
        let mode_label = match self.view_mode {
            ViewMode::Tree => "flat",
            ViewMode::Flat => "tree",
        };
        let footer = Paragraph::new(Line::from(vec![
            Span::styled("↑↓", Style::default().bold()),
            Span::raw(": Navigate | "),
            Span::styled("t", Style::default().bold()),
            Span::raw(format!(": Switch to {mode_label} | ")),
            Span::styled("q", Style::default().bold()),
            Span::raw(": Quit"),
        ]))
        .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(footer, area);
    }

    fn handle_events(&mut self) -> anyhow::Result<()> {
        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                return Ok(());
            }
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
                KeyCode::Down | KeyCode::Char('j') => self.next_row(),
                KeyCode::Up | KeyCode::Char('k') => self.prev_row(),
                KeyCode::Char('t') => self.toggle_view(),
                _ => {}
            }
        }
        Ok(())
    }

    fn toggle_view(&mut self) {
        self.view_mode = match self.view_mode {
            ViewMode::Tree => ViewMode::Flat,
            ViewMode::Flat => ViewMode::Tree,
        };
    }

    fn next_row(&mut self) {
        let total = self.row_count();
        if total == 0 {
            return;
        }
        let next = match self.table_state.selected() {
            Some(i) => (i + 1) % total,
            None => 0,
        };
        self.table_state.select(Some(next));
    }

    fn prev_row(&mut self) {
        let total = self.row_count();
        if total == 0 {
            return;
        }
        let prev = match self.table_state.selected() {
            Some(0) => total - 1,
            Some(i) => i - 1,
            None => total - 1,
        };
        self.table_state.select(Some(prev));
    }

    fn row_count(&self) -> usize {
        if self.stats.is_empty() {
            return 0;
        }
        let data_rows: usize = self.stats.iter().map(|p| 1 + p.by_language.len()).sum();
        data_rows + 2 + self.totals.by_language.len()
    }
}

pub fn run_tui(stats: &[PeriodStats], totals: &PeriodStats) -> anyhow::Result<()> {
    let mut terminal = ratatui::init();
    let mut app = TuiApp::new(stats.to_vec(), totals.clone());
    let result = app.run(&mut terminal);
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use crate::stats::models::LangStats;

    fn make_period(label: &str, commits: u64, adds: u64, dels: u64) -> PeriodStats {
        let mut by_language = HashMap::new();
        if adds > 0 || dels > 0 {
            by_language.insert(
                "Rust".to_string(),
                LangStats {
                    additions: adds,
                    deletions: dels,
                    files_changed: 1,
                    ..Default::default()
                },
            );
        }
        PeriodStats {
            period_label: label.to_string(),
            by_language,
            by_author: HashMap::new(),
            total_commits: commits,
            total_additions: adds,
            total_deletions: dels,
            total_net_modifications: adds.max(dels),
            total_net_additions: adds.saturating_sub(dels),
        }
    }

    fn make_multi_lang_period(label: &str) -> PeriodStats {
        let mut by_language = HashMap::new();
        by_language.insert(
            "Go".to_string(),
            LangStats {
                additions: 200,
                deletions: 50,
                files_changed: 5,
                net_modifications: 200,
                net_additions: 150,
            },
        );
        by_language.insert(
            "Python".to_string(),
            LangStats {
                additions: 100,
                deletions: 30,
                files_changed: 3,
                net_modifications: 100,
                net_additions: 70,
            },
        );
        PeriodStats {
            period_label: label.to_string(),
            by_language,
            by_author: HashMap::new(),
            total_commits: 10,
            total_additions: 300,
            total_deletions: 80,
            total_net_modifications: 300,
            total_net_additions: 220,
        }
    }

    fn make_totals() -> PeriodStats {
        make_period("Total", 15, 250, 40)
    }

    fn sample_stats() -> Vec<PeriodStats> {
        vec![
            make_period("2025-W01", 5, 100, 20),
            make_period("2025-W02", 10, 150, 20),
        ]
    }

    #[test]
    fn should_quit_defaults_to_false() {
        let app = TuiApp::new(sample_stats(), make_totals());
        assert!(!app.should_quit);
    }

    #[test]
    fn next_row_wraps_at_end() {
        let mut app = TuiApp::new(sample_stats(), make_totals());
        let total = app.row_count();
        assert_eq!(app.table_state.selected(), Some(0));
        for i in 1..total {
            app.next_row();
            assert_eq!(app.table_state.selected(), Some(i));
        }
        app.next_row();
        assert_eq!(app.table_state.selected(), Some(0));
    }

    #[test]
    fn prev_row_wraps_at_beginning() {
        let mut app = TuiApp::new(sample_stats(), make_totals());
        let total = app.row_count();
        assert_eq!(app.table_state.selected(), Some(0));
        app.prev_row();
        assert_eq!(app.table_state.selected(), Some(total - 1));
    }

    #[test]
    fn navigation_on_empty_stats_does_not_panic() {
        let totals = make_period("Total", 0, 0, 0);
        let mut app = TuiApp::new(vec![], totals);
        assert_eq!(app.table_state.selected(), None);
        app.next_row();
        app.prev_row();
        assert_eq!(app.table_state.selected(), None);
    }

    #[test]
    fn top_language_returns_highest_additions() {
        let mut by_language = HashMap::new();
        by_language.insert(
            "Rust".to_string(),
            LangStats {
                additions: 200,
                deletions: 10,
                files_changed: 3,
                net_modifications: 200,
                net_additions: 190,
            },
        );
        by_language.insert(
            "Python".to_string(),
            LangStats {
                additions: 50,
                deletions: 5,
                files_changed: 1,
                net_modifications: 50,
                net_additions: 45,
            },
        );
        let period = PeriodStats {
            period_label: "test".to_string(),
            by_language,
            by_author: HashMap::new(),
            total_commits: 5,
            total_additions: 250,
            total_deletions: 15,
            total_net_modifications: 250,
            total_net_additions: 235,
        };
        assert_eq!(top_language(&period), "Rust");
    }

    #[test]
    fn top_language_empty_returns_dash() {
        let period = PeriodStats {
            period_label: "test".to_string(),
            by_language: HashMap::new(),
            by_author: HashMap::new(),
            total_commits: 0,
            total_additions: 0,
            total_deletions: 0,
            total_net_modifications: 0,
            total_net_additions: 0,
        };
        assert_eq!(top_language(&period), "—");
    }

    #[test]
    fn row_count_includes_language_rows() {
        let stats = vec![make_multi_lang_period("2025-W01")];
        let totals = make_totals();
        let app = TuiApp::new(stats, totals);
        assert_eq!(app.row_count(), 6);
    }

    #[test]
    fn row_count_empty_is_zero() {
        let totals = make_period("Total", 0, 0, 0);
        let app = TuiApp::new(vec![], totals);
        assert_eq!(app.row_count(), 0);
    }

    #[test]
    fn sorted_languages_returns_by_total_desc() {
        let period = make_multi_lang_period("test");
        let langs = sorted_languages(&period);
        assert_eq!(langs.len(), 2);
        assert_eq!(langs[0].0, "Go");
        assert_eq!(langs[1].0, "Python");
    }

    #[test]
    fn toggle_view_switches_mode() {
        let mut app = TuiApp::new(sample_stats(), make_totals());
        assert_eq!(app.view_mode, ViewMode::Tree);
        app.toggle_view();
        assert_eq!(app.view_mode, ViewMode::Flat);
        app.toggle_view();
        assert_eq!(app.view_mode, ViewMode::Tree);
    }
}
