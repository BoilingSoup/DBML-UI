//! Diagram rendering backends for export: SVG generation and PNG
//! rasterization. (On-screen rendering is the iced canvas; this module
//! feeds `:export`.)

pub mod raster;
pub mod svg;
