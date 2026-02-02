use ratatui::style::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeVariant {
    Zinc,
    Nord,
    Cyberpunk,
    SolarizedDark,
}

impl ThemeVariant {
    pub fn cycle(&self) -> Self {
        match self {
            Self::Zinc => Self::Nord,
            Self::Nord => Self::Cyberpunk,
            Self::Cyberpunk => Self::SolarizedDark,
            Self::SolarizedDark => Self::Zinc,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Zinc => "Zinc",
            Self::Nord => "Nord",
            Self::Cyberpunk => "Cyberpunk",
            Self::SolarizedDark => "Solarized Dark",
        }
    }
}

pub struct Theme {
    pub variant: ThemeVariant,
    pub bg_primary: Color,
    pub bg_secondary: Color,
    pub text_main: Color,
    pub text_muted: Color,
    pub border_focus: Color,
    pub border_dim: Color,
    pub status_success: Color,
    pub status_warning: Color,
    pub status_error: Color,
    pub status_info: Color,
}

impl Theme {
    pub fn new(variant: ThemeVariant) -> Self {
        match variant {
            ThemeVariant::Zinc => Self {
                variant,
                // Using a "Zinc" inspired dark palette
                bg_primary: Color::Rgb(9, 9, 11), // Zinc 950
                bg_secondary: Color::Rgb(24, 24, 27), // Zinc 900
                
                text_main: Color::Rgb(244, 244, 245), // Zinc 100
                text_muted: Color::Rgb(161, 161, 170), // Zinc 400
                
                border_focus: Color::Rgb(63, 63, 70), // Zinc 700
                border_dim: Color::Rgb(39, 39, 42), // Zinc 800
                
                // Accents
                status_success: Color::Rgb(34, 197, 94), // Green 500
                status_warning: Color::Rgb(234, 179, 8), // Yellow 500
                status_error: Color::Rgb(239, 68, 68), // Red 500
                status_info: Color::Rgb(59, 130, 246), // Blue 500
            },
            ThemeVariant::Nord => Self {
                variant,
                bg_primary: Color::Rgb(46, 52, 64),    // nord0
                bg_secondary: Color::Rgb(59, 66, 82),  // nord1
                text_main: Color::Rgb(236, 239, 244),  // nord6
                text_muted: Color::Rgb(216, 222, 233), // nord4
                border_focus: Color::Rgb(136, 192, 208), // nord8
                border_dim: Color::Rgb(76, 86, 106),   // nord3
                status_success: Color::Rgb(163, 190, 140), // nord14
                status_warning: Color::Rgb(235, 203, 139), // nord13
                status_error: Color::Rgb(191, 97, 106),    // nord11
                status_info: Color::Rgb(94, 129, 172),     // nord10
            },
            ThemeVariant::Cyberpunk => Self {
                variant,
                bg_primary: Color::Rgb(10, 10, 15),
                bg_secondary: Color::Rgb(30, 30, 40),
                text_main: Color::Rgb(255, 0, 255), // Neon Pink
                text_muted: Color::Rgb(0, 255, 255), // Cyan
                border_focus: Color::Rgb(255, 255, 0), // Yellow
                border_dim: Color::Rgb(100, 0, 100),
                status_success: Color::Rgb(0, 255, 100),
                status_warning: Color::Rgb(255, 150, 0),
                status_error: Color::Rgb(255, 0, 50),
                status_info: Color::Rgb(0, 200, 255),
            },
            ThemeVariant::SolarizedDark => Self {
                variant,
                bg_primary: Color::Rgb(0, 43, 54),     // base03
                bg_secondary: Color::Rgb(7, 54, 66),   // base02
                text_main: Color::Rgb(131, 148, 150),  // base0
                text_muted: Color::Rgb(88, 110, 117),  // base01
                border_focus: Color::Rgb(42, 161, 152), // cyan
                border_dim: Color::Rgb(7, 54, 66),      // base02
                status_success: Color::Rgb(133, 153, 0),  // green
                status_warning: Color::Rgb(181, 137, 0),  // yellow
                status_error: Color::Rgb(220, 50, 47),    // red
                status_info: Color::Rgb(38, 139, 210),    // blue
            },
        }
    }
    
    pub fn default() -> Self {
        Self::new(ThemeVariant::Zinc)
    }
}
