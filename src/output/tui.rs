use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState},
};

use crate::cli::NumberFormat;
use crate::output::column::{DisplayCol, build_display_cols, format_presentation_label};
use crate::output::presentation::{PresentationModel, PresentationRow, PresentationRowKind};
use crate::output::table::format_num;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ViewMode {
    Tree,
    Flat,
}

pub struct TuiApp {
    model: PresentationModel,
    table_state: TableState,
    should_quit: bool,
    view_mode: ViewMode,
    num_fmt: NumberFormat,
    compact: bool,
}

impl TuiApp {
    pub fn new(model: PresentationModel, num_fmt: NumberFormat, compact: bool) -> Self {
        let mut table_state = TableState::default();
        if !model.rows.is_empty() {
            table_state.select(Some(0));
        }
        let view_mode = if model.inline_tree {
            ViewMode::Tree
        } else {
            ViewMode::Flat
        };
        Self {
            model,
            table_state,
            should_quit: false,
            view_mode,
            num_fmt,
            compact,
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
        let header = Paragraph::new(Line::from(vec![
            Span::styled(
                "logit — lines of git",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("  v{}", env!("CARGO_PKG_VERSION"))),
        ]));
        frame.render_widget(header, area);
    }

    fn render_table(&mut self, frame: &mut Frame, area: Rect) {
        let display_columns = build_display_cols(&self.model.columns, self.compact);
        let mut headers = Vec::with_capacity(display_columns.len() + 1);
        headers.push(Cell::from(self.model.label_header.clone()));
        headers.extend(
            display_columns
                .iter()
                .map(|column| Cell::from(column.header())),
        );
        let header = Row::new(headers)
            .style(Style::default().bold())
            .bottom_margin(1);

        let mut widths = Vec::with_capacity(display_columns.len() + 1);
        widths.push(Constraint::Min(self.model.label_header.len().max(16) as u16));
        widths.extend(
            display_columns
                .iter()
                .map(|column| Constraint::Length((column.header().len().max(8) + 1) as u16)),
        );

        let mut rows: Vec<Row> = self
            .model
            .rows
            .iter()
            .enumerate()
            .map(|(index, row)| {
                presentation_row(
                    row,
                    presentation_label(&self.model, index, row, self.view_mode, self.num_fmt),
                    &display_columns,
                    self.num_fmt,
                )
            })
            .collect();
        rows.push(presentation_row(
            &self.model.total,
            format_presentation_label(&self.model.total, &self.model.columns, self.num_fmt),
            &display_columns,
            self.num_fmt,
        ));

        let title = match self.view_mode {
            ViewMode::Tree => "Stats [tree]",
            ViewMode::Flat => "Stats [flat]",
        };
        let table = Table::new(rows, widths)
            .header(header)
            .block(Block::default().borders(Borders::ALL).title(title))
            .row_highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol(">> ");
        frame.render_stateful_widget(table, area, &mut self.table_state);
    }

    fn render_footer(&self, frame: &mut Frame, area: Rect) {
        let next_mode = match self.view_mode {
            ViewMode::Tree => "flat",
            ViewMode::Flat => "tree",
        };
        let footer = Paragraph::new(Line::from(vec![
            Span::styled("↑↓", Style::default().bold()),
            Span::raw(": Navigate | "),
            Span::styled("t", Style::default().bold()),
            Span::raw(format!(": Switch to {next_mode} | ")),
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
        let next = self
            .table_state
            .selected()
            .map_or(0, |index| (index + 1) % total);
        self.table_state.select(Some(next));
    }

    fn prev_row(&mut self) {
        let total = self.row_count();
        if total == 0 {
            return;
        }
        let previous = match self.table_state.selected() {
            Some(0) | None => total - 1,
            Some(index) => index - 1,
        };
        self.table_state.select(Some(previous));
    }

    fn row_count(&self) -> usize {
        if self.model.rows.is_empty() {
            0
        } else {
            self.model.rows.len() + 1
        }
    }
}

fn presentation_row<'a>(
    row: &PresentationRow,
    label: String,
    columns: &[DisplayCol],
    num_fmt: NumberFormat,
) -> Row<'a> {
    let base_style = match row.kind {
        PresentationRowKind::Group => Style::default(),
        PresentationRowKind::Language => Style::default().fg(Color::DarkGray),
        PresentationRowKind::Total => Style::default().add_modifier(Modifier::BOLD),
    };
    let mut cells = Vec::with_capacity(columns.len() + 1);
    cells.push(Cell::from(label));
    cells.extend(
        columns
            .iter()
            .map(|column| metric_cell(*column, row, num_fmt)),
    );
    Row::new(cells).style(base_style)
}

fn metric_cell<'a>(column: DisplayCol, row: &PresentationRow, num_fmt: NumberFormat) -> Cell<'a> {
    let metrics = row.metrics;
    match column {
        DisplayCol::Commits => Cell::from(format_num(metrics.commits, num_fmt)),
        DisplayCol::Adds => Cell::from(format_num(metrics.additions, num_fmt))
            .style(Style::default().fg(Color::Green)),
        DisplayCol::Dels => Cell::from(format_num(metrics.deletions, num_fmt))
            .style(Style::default().fg(Color::Red)),
        DisplayCol::Changes => Cell::from(format!(
            "+{} -{}",
            format_num(metrics.additions, num_fmt),
            format_num(metrics.deletions, num_fmt)
        )),
        DisplayCol::Net => {
            let net = metrics.net();
            let sign = if net >= 0 { '+' } else { '-' };
            let color = if net >= 0 { Color::Green } else { Color::Red };
            Cell::from(format!("{sign}{}", format_num(net.unsigned_abs(), num_fmt)))
                .style(Style::default().fg(color))
        }
        DisplayCol::Files => {
            Cell::from(format_num(metrics.files, num_fmt)).style(Style::default().fg(Color::Yellow))
        }
    }
}

fn presentation_label(
    model: &PresentationModel,
    index: usize,
    row: &PresentationRow,
    view_mode: ViewMode,
    num_fmt: NumberFormat,
) -> String {
    let label = format_presentation_label(row, &model.columns, num_fmt);
    if row.depth == 0 {
        return label;
    }
    if view_mode == ViewMode::Flat {
        return format!("{}{label}", "  ".repeat(row.depth));
    }
    let next_boundary = model.rows[index + 1..]
        .iter()
        .find(|next| next.depth <= row.depth);
    let is_last = next_boundary.is_none_or(|next| next.depth < row.depth);
    let branch = if is_last { "└── " } else { "├── " };
    format!("{}{branch}{label}", "    ".repeat(row.depth - 1))
}

pub fn run_tui(
    model: &PresentationModel,
    num_fmt: NumberFormat,
    compact: bool,
) -> anyhow::Result<()> {
    let mut terminal = ratatui::init();
    let mut app = TuiApp::new(model.clone(), num_fmt, compact);
    let result = app.run(&mut terminal);
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend};

    use super::*;
    use crate::cli::Column;
    use crate::output::presentation::{PresentationMetrics, PresentationRow};

    fn sample_model() -> PresentationModel {
        PresentationModel {
            label_header: "Period / Language".to_string(),
            columns: Column::default_set(),
            rows: vec![
                PresentationRow {
                    depth: 0,
                    label: "2025-W01".to_string(),
                    kind: PresentationRowKind::Group,
                    metrics: PresentationMetrics {
                        commits: 5,
                        additions: 100,
                        deletions: 20,
                        files: 2,
                    },
                },
                PresentationRow {
                    depth: 1,
                    label: "Rust".to_string(),
                    kind: PresentationRowKind::Language,
                    metrics: PresentationMetrics {
                        additions: 100,
                        deletions: 20,
                        files: 2,
                        ..Default::default()
                    },
                },
            ],
            total: PresentationRow {
                depth: 0,
                label: "Total".to_string(),
                kind: PresentationRowKind::Total,
                metrics: PresentationMetrics {
                    commits: 5,
                    additions: 100,
                    deletions: 20,
                    files: 2,
                },
            },
            inline_tree: true,
        }
    }

    fn render_to_text(app: &mut TuiApp) -> anyhow::Result<String> {
        let backend = TestBackend::new(120, 20);
        let mut terminal = Terminal::new(backend)?;
        terminal.draw(|frame| app.render(frame))?;
        let buffer = terminal.backend().buffer();
        Ok(buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>())
    }

    #[test]
    fn should_quit_defaults_to_false() {
        let app = TuiApp::new(sample_model(), NumberFormat::Separated, true);
        assert!(!app.should_quit);
    }

    #[test]
    fn next_row_wraps_at_end() {
        let mut app = TuiApp::new(sample_model(), NumberFormat::Separated, true);
        let total = app.row_count();
        for expected in 1..total {
            app.next_row();
            assert_eq!(app.table_state.selected(), Some(expected));
        }
        app.next_row();
        assert_eq!(app.table_state.selected(), Some(0));
    }

    #[test]
    fn prev_row_wraps_at_beginning() {
        let mut app = TuiApp::new(sample_model(), NumberFormat::Separated, true);
        app.prev_row();
        assert_eq!(app.table_state.selected(), Some(app.row_count() - 1));
    }

    #[test]
    fn navigation_on_empty_stats_does_not_panic() {
        let mut model = sample_model();
        model.rows.clear();
        let mut app = TuiApp::new(model, NumberFormat::Separated, true);
        app.next_row();
        app.prev_row();
        assert_eq!(app.table_state.selected(), None);
    }

    #[test]
    fn row_count_includes_language_rows() {
        let app = TuiApp::new(sample_model(), NumberFormat::Separated, true);
        assert_eq!(app.row_count(), app.model.rows.len() + 1);
    }

    #[test]
    fn row_count_empty_is_zero() {
        let mut model = sample_model();
        model.rows.clear();
        let app = TuiApp::new(model, NumberFormat::Separated, true);
        assert_eq!(app.row_count(), 0);
    }

    #[test]
    fn toggle_view_switches_mode() {
        let mut app = TuiApp::new(sample_model(), NumberFormat::Separated, true);
        assert_eq!(app.view_mode, ViewMode::Tree);
        app.toggle_view();
        assert_eq!(app.view_mode, ViewMode::Flat);
        app.toggle_view();
        assert_eq!(app.view_mode, ViewMode::Tree);
    }

    #[test]
    fn tui_multigroup_is_nonempty_and_successful() {
        let mut model = sample_model();
        model.label_header = "Repo / Author / Language".to_string();
        model.rows.insert(
            1,
            PresentationRow {
                depth: 1,
                label: "Alice".to_string(),
                kind: PresentationRowKind::Group,
                metrics: model.rows[0].metrics,
            },
        );
        model.rows[0].label = "repo-a".to_string();
        model.rows[2].depth = 2;
        let mut app = TuiApp::new(model, NumberFormat::Plain, false);
        let output = render_to_text(&mut app).expect("render multi-group TUI");
        assert!(app.row_count() > 1);
        assert!(output.contains("repo-a"));
        assert!(output.contains("Alice"));
        assert!(output.contains("Rust"));
    }

    #[test]
    fn tui_has_no_fixed_period_or_five_metric_assumption() {
        let mut model = sample_model();
        model.label_header = "Repository".to_string();
        model.columns = vec![Column::Files, Column::Net];
        model.rows[0].label = "repo-a".to_string();
        let mut app = TuiApp::new(model, NumberFormat::Plain, false);
        let output = render_to_text(&mut app).expect("render selected TUI columns");
        assert!(output.contains("Repository"));
        assert!(output.contains("Files"));
        assert!(output.contains("Net"));
        assert!(!output.contains("Period"));
        assert!(!output.contains("Commits"));
        assert!(!output.contains("Additions"));
    }

    #[test]
    fn table_and_tui_testbackend_show_same_semantic_rows() {
        let model = PresentationModel {
            label_header: "Repo / Author / Language".to_string(),
            columns: vec![Column::Files, Column::Commits, Column::Net],
            rows: vec![
                PresentationRow {
                    depth: 0,
                    label: "repo-a".to_string(),
                    kind: PresentationRowKind::Group,
                    metrics: PresentationMetrics {
                        commits: 2,
                        additions: 9,
                        deletions: 2,
                        files: 1,
                    },
                },
                PresentationRow {
                    depth: 1,
                    label: "Alice".to_string(),
                    kind: PresentationRowKind::Group,
                    metrics: PresentationMetrics {
                        commits: 2,
                        additions: 9,
                        deletions: 2,
                        files: 1,
                    },
                },
                PresentationRow {
                    depth: 2,
                    label: "Rust".to_string(),
                    kind: PresentationRowKind::Language,
                    metrics: PresentationMetrics {
                        additions: 9,
                        deletions: 2,
                        files: 1,
                        ..Default::default()
                    },
                },
            ],
            total: PresentationRow {
                depth: 0,
                label: "Total".to_string(),
                kind: PresentationRowKind::Total,
                metrics: PresentationMetrics {
                    commits: 2,
                    additions: 9,
                    deletions: 2,
                    files: 1,
                },
            },
            inline_tree: true,
        };
        colored::control::set_override(false);
        let table =
            crate::output::table::render_presentation_table(&model, NumberFormat::Plain, false);
        let mut app = TuiApp::new(model, NumberFormat::Plain, false);
        let tui = render_to_text(&mut app).expect("render TUI test backend");
        for semantic in [
            "repo-a", "Alice", "Rust", "Files", "Commits", "Net", "Total",
        ] {
            assert!(
                table.contains(semantic),
                "table missing {semantic}: {table}"
            );
            assert!(tui.contains(semantic), "TUI missing {semantic}: {tui}");
        }
    }
}
