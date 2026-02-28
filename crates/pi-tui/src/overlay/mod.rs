use crate::components::traits::Component;

/// How the overlay is positioned relative to the terminal.
#[derive(Debug, Clone)]
pub enum OverlayAnchor {
    Center,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    TopCenter,
    BottomCenter,
}

/// A dimension value — either an absolute pixel count or a percentage of the
/// available terminal dimension.
#[derive(Debug, Clone)]
pub enum SizeValue {
    Absolute(u16),
    Percent(f32),
}

impl SizeValue {
    pub fn resolve(&self, available: u16) -> u16 {
        match self {
            SizeValue::Absolute(n) => *n,
            SizeValue::Percent(p) => ((available as f32) * p / 100.0).round() as u16,
        }
    }
}

/// Options that control how an overlay is positioned and sized.
#[derive(Debug, Clone)]
pub struct OverlayOptions {
    /// Desired width. If `None`, the overlay uses the component's natural width.
    pub width: Option<SizeValue>,
    /// Minimum width. Enforced when the computed width would be smaller.
    pub min_width: Option<u16>,
    /// Maximum height. Overlay content is clipped or scrolled beyond this.
    pub max_height: Option<SizeValue>,
    /// Anchor point for positioning.
    pub anchor: OverlayAnchor,
    /// Horizontal offset from the anchor (positive = right).
    pub offset_x: i16,
    /// Vertical offset from the anchor (positive = down).
    pub offset_y: i16,
}

impl Default for OverlayOptions {
    fn default() -> Self {
        Self {
            width: None,
            min_width: None,
            max_height: None,
            anchor: OverlayAnchor::Center,
            offset_x: 0,
            offset_y: 0,
        }
    }
}

/// A handle returned when an overlay is shown. Can be used to hide or query
/// the overlay later.
pub struct OverlayHandle {
    #[allow(dead_code)]
    id: u64,
}

struct OverlayEntry {
    component: Box<dyn Component>,
    options: OverlayOptions,
    hidden: bool,
}

/// Manages a single active overlay (popup, modal, autocomplete dropdown, etc.).
///
/// Only one overlay can be active at a time. Showing a new overlay replaces
/// the previous one.
pub struct OverlayManager {
    overlay: Option<OverlayEntry>,
    next_id: u64,
}

impl OverlayManager {
    pub fn new() -> Self {
        Self {
            overlay: None,
            next_id: 0,
        }
    }

    /// Show `component` as the active overlay with the given options.
    /// Returns a handle that can be used to dismiss the overlay.
    pub fn show(
        &mut self,
        component: Box<dyn Component>,
        options: OverlayOptions,
    ) -> OverlayHandle {
        let id = self.next_id;
        self.next_id += 1;
        self.overlay = Some(OverlayEntry {
            component,
            options,
            hidden: false,
        });
        OverlayHandle { id }
    }

    /// Hide the current overlay.
    pub fn hide(&mut self) {
        self.overlay = None;
    }

    /// Returns `true` if there is an active, visible overlay.
    pub fn has_overlay(&self) -> bool {
        self.overlay.as_ref().map(|e| !e.hidden).unwrap_or(false)
    }

    /// Render the overlay and return a list of `(col, row, lines)` tuples
    /// describing where each batch of lines should be placed on screen.
    ///
    /// The caller (rendering engine) is responsible for writing these to the
    /// terminal at the specified positions.
    pub fn render(
        &self,
        terminal_width: u16,
        terminal_height: u16,
    ) -> Vec<(u16, u16, Vec<String>)> {
        let entry = match &self.overlay {
            Some(e) if !e.hidden => e,
            _ => return vec![],
        };

        // Resolve width
        let overlay_width = match &entry.options.width {
            Some(sv) => {
                let w = sv.resolve(terminal_width);
                if let Some(min_w) = entry.options.min_width {
                    w.max(min_w)
                } else {
                    w
                }
            }
            None => {
                let natural = entry.component.render(terminal_width);
                let max_len = natural
                    .iter()
                    .map(|l| {
                        // Approximate visual width by stripping ANSI and measuring
                        l.chars().filter(|c| !c.is_control()).count() as u16
                    })
                    .max()
                    .unwrap_or(terminal_width / 2);
                if let Some(min_w) = entry.options.min_width {
                    max_len.max(min_w)
                } else {
                    max_len
                }
            }
        }
        .min(terminal_width);

        let mut lines = entry.component.render(overlay_width);

        // Clamp height
        if let Some(max_h) = &entry.options.max_height {
            let max = max_h.resolve(terminal_height) as usize;
            lines.truncate(max);
        }

        let overlay_height = lines.len() as u16;

        // Compute top-left corner based on anchor
        let (base_col, base_row) = match entry.options.anchor {
            OverlayAnchor::Center => (
                (terminal_width.saturating_sub(overlay_width)) / 2,
                (terminal_height.saturating_sub(overlay_height)) / 2,
            ),
            OverlayAnchor::TopLeft => (0, 0),
            OverlayAnchor::TopRight => (terminal_width.saturating_sub(overlay_width), 0),
            OverlayAnchor::BottomLeft => (0, terminal_height.saturating_sub(overlay_height)),
            OverlayAnchor::BottomRight => (
                terminal_width.saturating_sub(overlay_width),
                terminal_height.saturating_sub(overlay_height),
            ),
            OverlayAnchor::TopCenter => ((terminal_width.saturating_sub(overlay_width)) / 2, 0),
            OverlayAnchor::BottomCenter => (
                (terminal_width.saturating_sub(overlay_width)) / 2,
                terminal_height.saturating_sub(overlay_height),
            ),
        };

        // Apply offsets (clamped to terminal bounds)
        let col = (base_col as i32 + entry.options.offset_x as i32)
            .clamp(0, (terminal_width.saturating_sub(overlay_width)) as i32)
            as u16;
        let row = (base_row as i32 + entry.options.offset_y as i32)
            .clamp(0, (terminal_height.saturating_sub(overlay_height)) as i32)
            as u16;

        vec![(col, row, lines)]
    }

    /// Forward input to the overlay's component if one is active.
    pub fn handle_input(&mut self, data: &str) -> crate::components::traits::InputResult {
        if let Some(ref mut entry) = self.overlay {
            if !entry.hidden {
                return entry.component.handle_input(data);
            }
        }
        crate::components::traits::InputResult::Ignored
    }
}

impl Default for OverlayManager {
    fn default() -> Self {
        Self::new()
    }
}
