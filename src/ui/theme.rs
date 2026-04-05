use ratatui::style::Color;

pub struct Theme {
    // ── Chrome / UI ──────────────────────────────────────────────────
    pub border: Color,
    pub border_highlight: Color,
    pub text_primary: Color,
    pub text_secondary: Color,
    pub text_dim: Color,
    pub divider: Color,
    pub empty_bar: Color,
    pub chart_bg: Color,
    pub dot_empty: Color,

    // ── Heatmap ──────────────────────────────────────────────────────
    pub heatmap_title: Color,
    pub heatmap_label: Color,
    pub input_colors: [Color; 5],
    pub output_colors: [Color; 5],
    pub lines_colors: [Color; 5],
    pub rate_colors: [Color; 5],

    // ── Accent colors ────────────────────────────────────────────────
    pub cost: Color,
    pub tokens_in: Color,
    pub tokens_out: Color,
    pub cache: Color,
    pub lines_positive: Color,
    pub lines_negative: Color,
    pub duration: Color,
    pub efficiency_accent: Color,
    pub error: Color,
    pub warning: Color,

    // ── Models ───────────────────────────────────────────────────────
    pub model_opus: Color,
    pub model_sonnet: Color,
    pub model_haiku: Color,
    pub model_other: Color,
    pub model_bar_text: Color,

    // ── Title & star animation ───────────────────────────────────────
    pub title: Color,
    pub star_base: (u8, u8, u8),
    pub star_amplitude: (f32, f32, f32),

    // ── Scanner separator base (faded end) ───────────────────────────
    pub scanner_base: (u8, u8, u8),

    // ── Rainbow (card starred) ───────────────────────────────────────
    pub rainbow: [Color; 6],
}

static THEME: Theme = Theme::dark();

pub fn theme() -> &'static Theme {
    &THEME
}

impl Theme {
    const fn dark() -> Self {
        Self {
            // Chrome
            border: Color::Rgb(60, 60, 65),
            border_highlight: Color::Rgb(240, 180, 50),
            text_primary: Color::White,
            text_secondary: Color::Rgb(200, 200, 205),
            text_dim: Color::White,
            divider: Color::Rgb(50, 50, 55),
            empty_bar: Color::Rgb(50, 50, 55),
            chart_bg: Color::Rgb(30, 30, 35),
            dot_empty: Color::White,

            // Heatmap
            heatmap_title: Color::White,
            heatmap_label: Color::White,
            input_colors: [
                Color::Rgb(22, 22, 26),
                Color::Rgb(20, 40, 60),
                Color::Rgb(30, 70, 110),
                Color::Rgb(50, 120, 180),
                Color::Rgb(80, 170, 240),
            ],
            output_colors: [
                Color::Rgb(22, 22, 26),
                Color::Rgb(42, 30, 92),
                Color::Rgb(76, 46, 138),
                Color::Rgb(132, 72, 186),
                Color::Rgb(190, 120, 240),
            ],
            lines_colors: [
                Color::Rgb(22, 22, 26),
                Color::Rgb(60, 40, 20),
                Color::Rgb(110, 70, 25),
                Color::Rgb(180, 110, 40),
                Color::Rgb(240, 160, 60),
            ],
            rate_colors: [
                Color::Rgb(22, 22, 26),
                Color::Rgb(140, 40, 30),
                Color::Rgb(170, 100, 20),
                Color::Rgb(120, 160, 40),
                Color::Rgb(60, 190, 80),
            ],

            // Accents
            cost: Color::Rgb(240, 180, 50),
            tokens_in: Color::Rgb(80, 170, 240),
            tokens_out: Color::Rgb(190, 120, 240),
            cache: Color::Rgb(120, 200, 160),
            lines_positive: Color::Rgb(60, 190, 80),
            lines_negative: Color::Red,
            duration: Color::Rgb(100, 200, 200),
            efficiency_accent: Color::Rgb(220, 160, 60),
            error: Color::Rgb(240, 80, 60),
            warning: Color::Yellow,

            // Models
            model_opus: Color::Rgb(190, 120, 240),
            model_sonnet: Color::Rgb(80, 170, 240),
            model_haiku: Color::Rgb(120, 200, 160),
            model_other: Color::Rgb(100, 100, 105),
            model_bar_text: Color::Rgb(20, 20, 25),

            // Title & star
            title: Color::White,
            star_base: (200, 120, 30),
            star_amplitude: (55.0, 60.0, 30.0),

            // Scanner
            scanner_base: (30, 30, 35),

            // Rainbow
            rainbow: [
                Color::Rgb(255, 80, 80),
                Color::Rgb(255, 160, 50),
                Color::Rgb(255, 230, 50),
                Color::Rgb(80, 220, 80),
                Color::Rgb(80, 170, 240),
                Color::Rgb(190, 120, 240),
            ],
        }
    }

    pub fn model_color(&self, model: &str) -> Color {
        match model {
            "opus" => self.model_opus,
            "sonnet" => self.model_sonnet,
            "haiku" => self.model_haiku,
            _ => self.model_other,
        }
    }
}
