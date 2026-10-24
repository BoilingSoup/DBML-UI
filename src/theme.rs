#![allow(dead_code)] // consumed by the canvas milestones

//! Shared dark palette. The on-screen canvas and the SVG exporter use the
//! same values so `:export` matches what you see.

pub const BG: iced::Color = iced::Color::from_rgb(0.063, 0.071, 0.086); // #101216
pub const PANEL: iced::Color = iced::Color::from_rgb(0.11, 0.129, 0.157); // #1c2128
pub const BORDER: iced::Color = iced::Color::from_rgb(0.267, 0.298, 0.337); // #444c56
pub const TEXT: iced::Color = iced::Color::from_rgb(0.902, 0.929, 0.953); // #e6edf3
pub const DIM: iced::Color = iced::Color::from_rgb(0.545, 0.580, 0.620); // #8b949e

pub const TABLE_HEADER: iced::Color =
    iced::Color::from_rgb(47.0 / 255.0, 122.0 / 255.0, 220.0 / 255.0); // matches svg.rs header default
pub const ENUM_HEADER: iced::Color =
    iced::Color::from_rgb(145.0 / 255.0, 104.0 / 255.0, 219.0 / 255.0);

pub const PK: iced::Color = iced::Color::from_rgb(0.824, 0.6, 0.133); // ◆ #d29922
pub const FK: iced::Color = iced::Color::from_rgb(0.267, 0.576, 0.973); // ◇ #4493f8
pub const EDGE: iced::Color = iced::Color::from_rgb(0.431, 0.463, 0.506); // #6e7681

pub fn app() -> iced::Theme {
    iced::Theme::Dark
}
