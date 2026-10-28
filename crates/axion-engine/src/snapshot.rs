//! Saving a page as PDF and taking screenshots of it.

/// How a page is printed to PDF (all lengths in inches).
#[derive(Clone, Debug, PartialEq)]
pub struct PdfOptions {
    pub landscape: bool,
    pub print_background: bool,
    /// 1.0 is 100%.
    pub scale: f64,
    pub paper_width: f64,
    pub paper_height: f64,
    pub margin_top: f64,
    pub margin_right: f64,
    pub margin_bottom: f64,
    pub margin_left: f64,
    /// Title, date, address and page numbers above and below the content.
    pub header_footer: bool,
    /// "1-3, 5" (empty: all pages).
    pub page_ranges: String,
}

impl Default for PdfOptions {
    fn default() -> Self {
        Self {
            landscape: false,
            print_background: false,
            scale: 1.0,
            paper_width: 8.27,
            paper_height: 11.69,
            margin_top: 0.4,
            margin_right: 0.4,
            margin_bottom: 0.4,
            margin_left: 0.4,
            header_footer: false,
            page_ranges: String::new(),
        }
    }
}
