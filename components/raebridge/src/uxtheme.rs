//! UxTheme visual styles API emulation for RaeBridge.
#![no_std]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::{HResult, Rect, Size, WinHandle};

// ---------------------------------------------------------------------------
// HRESULT constants
// ---------------------------------------------------------------------------

pub const S_OK: i32 = 0;
pub const S_FALSE: i32 = 1;
pub const E_FAIL: i32 = -2147467259;
pub const E_INVALIDARG: i32 = -2147024809;
pub const E_HANDLE: i32 = -2147024890;

// ---------------------------------------------------------------------------
// Theme handle
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HTheme(pub u64);

pub const NULL_HTHEME: HTheme = HTheme(0);

// ---------------------------------------------------------------------------
// Open theme flags
// ---------------------------------------------------------------------------

pub const OTD_FORCE_RECT_SIZING: u32 = 0x1;
pub const OTD_NONCLIENT: u32 = 0x2;

// ---------------------------------------------------------------------------
// Theme parts
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum ThemePart {
    ButtonPushButton = 101,
    ButtonRadioButton = 102,
    ButtonCheckBox = 103,
    ButtonGroupBox = 104,
    ButtonCommandLink = 106,
    ComboboxDropdownButton = 201,
    ComboboxBackground = 202,
    EditBackground = 301,
    EditBorder = 303,
    HeaderItem = 401,
    HeaderSortArrow = 404,
    ListviewItem = 501,
    ListviewGroup = 502,
    ListviewGroupHeader = 512,
    MenuBarBackground = 607,
    MenuBarItem = 608,
    MenuItemPopupBackground = 609,
    MenuItemPopupItem = 614,
    MenuItemPopupSeparator = 615,
    ProgressBar = 701,
    ProgressFill = 705,
    RebarBackground = 806,
    RebarBand = 803,
    ScrollbarArrowBtn = 901,
    ScrollbarThumbBtnHorz = 902,
    ScrollbarThumbBtnVert = 903,
    ScrollbarTrackHorz = 904,
    ScrollbarTrackVert = 905,
    ScrollbarSizeBox = 909,
    SpinUp = 1001,
    SpinDown = 1002,
    TabItem = 1101,
    TabPane = 1109,
    TabBody = 1110,
    ToolbarButton = 1201,
    ToolbarSeparator = 1205,
    TooltipStandard = 1301,
    TooltipBallon = 1303,
    TrackbarTrack = 1401,
    TrackbarThumb = 1403,
    TrackbarThumbVert = 1406,
    TreeviewItem = 1501,
    TreeviewGlyph = 1502,
    WindowCaption = 1601,
    WindowFrame = 1607,
    WindowCloseButton = 1618,
    WindowMaxButton = 1617,
    WindowMinButton = 1615,
    WindowRestoreButton = 1621,
    DatePickerDateBorder = 1702,
    DatePickerDateText = 1704,
    FlyoutWindow = 1801,
    FlyoutHeader = 1803,
    NavigationBackButton = 1901,
    SearchEditboxBackground = 2001,
    TaskbarBackground = 2101,
    TaskbandButton = 2102,
    StartPanelMorePrograms = 2202,
}

// ---------------------------------------------------------------------------
// Theme states
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum ThemeState {
    Normal = 1,
    Hot = 2,
    Pressed = 3,
    Disabled = 4,
    Defaulted = 5,
    Focused = 6,
    ReadOnly = 7,
    Selected = 8,
    Mixed = 9,
    Indeterminate = 10,
    Partial = 11,
}

// ---------------------------------------------------------------------------
// Theme size type
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeSizeType {
    Min,
    True,
    Draw,
}

// ---------------------------------------------------------------------------
// Margins
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default)]
pub struct ThemeMargins {
    pub left: i32,
    pub right: i32,
    pub top: i32,
    pub bottom: i32,
}

// ---------------------------------------------------------------------------
// Theme font
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ThemeFont {
    pub face_name: String,
    pub height: i32,
    pub width: i32,
    pub weight: i32,
    pub italic: bool,
    pub underline: bool,
    pub strikeout: bool,
    pub char_set: u8,
    pub quality: u8,
}

// ---------------------------------------------------------------------------
// Theme int list
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ThemeIntList {
    pub values: Vec<i32>,
}

// ---------------------------------------------------------------------------
// Buffered paint
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HBufferedPaint(pub u64);

pub const NULL_HBUFFEREDPAINT: HBufferedPaint = HBufferedPaint(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferedPaintFormat {
    CompatibleBitmap,
    Dib,
    TopDownDib,
    TopDownMonoDib,
}

#[derive(Debug, Clone)]
pub struct BufferedPaintParams {
    pub size: u32,
    pub flags: u32,
    pub format: BufferedPaintFormat,
    pub paint_params: Option<WinHandle>,
    pub clip_rect: Option<Rect>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HBufferedAnimation(pub u64);

pub const NULL_HBUFFEREDANIMATION: HBufferedAnimation = HBufferedAnimation(0);

// ---------------------------------------------------------------------------
// Buffered paint state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct BufferedPaintState {
    pub handle: HBufferedPaint,
    pub hdc_target: WinHandle,
    pub hdc_buffer: WinHandle,
    pub rc_target: Rect,
    pub bits: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

// ---------------------------------------------------------------------------
// Theme data store
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ThemeData {
    pub handle: HTheme,
    pub class_name: String,
    pub hwnd: WinHandle,
    pub flags: u32,
    pub dpi: u32,
    pub colors: BTreeMap<(u32, u32, u32), u32>,
    pub sizes: BTreeMap<(u32, u32), Size>,
    pub booleans: BTreeMap<(u32, u32, u32), bool>,
    pub integers: BTreeMap<(u32, u32, u32), i32>,
    pub margins_map: BTreeMap<(u32, u32, u32), ThemeMargins>,
    pub fonts: BTreeMap<(u32, u32, u32), ThemeFont>,
    pub transition_durations: BTreeMap<(u32, u32, u32), u32>,
}

impl ThemeData {
    pub fn new(handle: HTheme, class_name: &str, hwnd: WinHandle, flags: u32) -> Self {
        Self {
            handle,
            class_name: String::from(class_name),
            hwnd,
            flags,
            dpi: 96,
            colors: BTreeMap::new(),
            sizes: BTreeMap::new(),
            booleans: BTreeMap::new(),
            integers: BTreeMap::new(),
            margins_map: BTreeMap::new(),
            fonts: BTreeMap::new(),
            transition_durations: BTreeMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// UxTheme API
// ---------------------------------------------------------------------------

pub struct UxThemeApi {
    pub initialized: bool,
    pub theme_active: bool,
    pub app_themed: bool,
    pub composition_active: bool,
    pub themes: BTreeMap<u64, ThemeData>,
    pub next_handle: u64,
    pub window_themes: BTreeMap<u64, String>,
    pub buffered_paint_init: bool,
    pub buffered_paints: BTreeMap<u64, BufferedPaintState>,
    pub next_bp_handle: u64,
    pub sys_colors: BTreeMap<i32, u32>,
    pub sys_fonts: BTreeMap<i32, ThemeFont>,
    pub sys_metrics: BTreeMap<i32, i32>,
    pub dpi: u32,
}

impl UxThemeApi {
    pub const fn new() -> Self {
        Self {
            initialized: false,
            theme_active: true,
            app_themed: true,
            composition_active: true,
            themes: BTreeMap::new(),
            next_handle: 0x7E_000001,
            window_themes: BTreeMap::new(),
            buffered_paint_init: false,
            buffered_paints: BTreeMap::new(),
            next_bp_handle: 0x7F_000001,
            sys_colors: BTreeMap::new(),
            sys_fonts: BTreeMap::new(),
            sys_metrics: BTreeMap::new(),
            dpi: 96,
        }
    }

    pub fn init(&mut self) {
        if self.initialized {
            return;
        }
        self.populate_sys_colors();
        self.populate_sys_metrics();
        self.initialized = true;
    }

    fn populate_sys_colors(&mut self) {
        self.sys_colors.insert(0, 0x00000000); // COLOR_SCROLLBAR
        self.sys_colors.insert(1, 0x00D4D0C8); // COLOR_BACKGROUND
        self.sys_colors.insert(2, 0x000A246A); // COLOR_ACTIVECAPTION
        self.sys_colors.insert(3, 0x00808080); // COLOR_INACTIVECAPTION
        self.sys_colors.insert(4, 0x00C0C0C0); // COLOR_MENU
        self.sys_colors.insert(5, 0x00FFFFFF); // COLOR_WINDOW
        self.sys_colors.insert(6, 0x00000000); // COLOR_WINDOWFRAME
        self.sys_colors.insert(7, 0x00000000); // COLOR_MENUTEXT
        self.sys_colors.insert(8, 0x00000000); // COLOR_WINDOWTEXT
        self.sys_colors.insert(9, 0x00FFFFFF); // COLOR_CAPTIONTEXT
        self.sys_colors.insert(13, 0x000078D7); // COLOR_HIGHLIGHT
        self.sys_colors.insert(14, 0x00FFFFFF); // COLOR_HIGHLIGHTTEXT
        self.sys_colors.insert(15, 0x00F0F0F0); // COLOR_3DFACE
    }

    fn populate_sys_metrics(&mut self) {
        self.sys_metrics.insert(0, 800); // SM_CXSCREEN
        self.sys_metrics.insert(1, 600); // SM_CYSCREEN
        self.sys_metrics.insert(2, 20); // SM_CXVSCROLL
        self.sys_metrics.insert(3, 20); // SM_CYHSCROLL
        self.sys_metrics.insert(4, 23); // SM_CYCAPTION
        self.sys_metrics.insert(5, 1); // SM_CXBORDER
        self.sys_metrics.insert(6, 1); // SM_CYBORDER
        self.sys_metrics.insert(32, 23); // SM_CYMENU
    }

    // --- Theme handle management ---

    pub fn open_theme_data(&mut self, hwnd: WinHandle, class_name: &str) -> HTheme {
        self.open_theme_data_ex(hwnd, class_name, 0)
    }

    pub fn open_theme_data_ex(&mut self, hwnd: WinHandle, class_name: &str, flags: u32) -> HTheme {
        if !self.theme_active {
            return NULL_HTHEME;
        }
        let id = self.next_handle;
        self.next_handle += 1;
        let handle = HTheme(id);
        let mut data = ThemeData::new(handle, class_name, hwnd, flags);
        populate_theme_defaults(&mut data);
        self.themes.insert(id, data);
        handle
    }

    pub fn close_theme_data(&mut self, theme: HTheme) -> HResult {
        if self.themes.remove(&theme.0).is_some() {
            HResult(S_OK)
        } else {
            HResult(E_HANDLE)
        }
    }

    // --- Drawing ---

    pub fn draw_theme_background(
        &self,
        theme: HTheme,
        _hdc: WinHandle,
        _part_id: u32,
        _state_id: u32,
        _rect: &Rect,
        _clip_rect: Option<&Rect>,
    ) -> HResult {
        if !self.themes.contains_key(&theme.0) {
            return HResult(E_HANDLE);
        }
        HResult(S_OK)
    }

    pub fn draw_theme_background_ex(
        &self,
        theme: HTheme,
        _hdc: WinHandle,
        _part_id: u32,
        _state_id: u32,
        _rect: &Rect,
        _options: &DrawThemeBackgroundOptions,
    ) -> HResult {
        if !self.themes.contains_key(&theme.0) {
            return HResult(E_HANDLE);
        }
        HResult(S_OK)
    }

    pub fn draw_theme_text(
        &self,
        theme: HTheme,
        _hdc: WinHandle,
        _part_id: u32,
        _state_id: u32,
        _text: &str,
        _flags: u32,
        _rect: &Rect,
    ) -> HResult {
        if !self.themes.contains_key(&theme.0) {
            return HResult(E_HANDLE);
        }
        HResult(S_OK)
    }

    pub fn draw_theme_text_ex(
        &self,
        theme: HTheme,
        _hdc: WinHandle,
        _part_id: u32,
        _state_id: u32,
        _text: &str,
        _flags: u32,
        _rect: &Rect,
        _options: &DrawThemeTextOptions,
    ) -> HResult {
        if !self.themes.contains_key(&theme.0) {
            return HResult(E_HANDLE);
        }
        HResult(S_OK)
    }

    pub fn draw_theme_icon(
        &self,
        theme: HTheme,
        _hdc: WinHandle,
        _part_id: u32,
        _state_id: u32,
        _rect: &Rect,
        _image_list: WinHandle,
        _image_index: i32,
    ) -> HResult {
        if !self.themes.contains_key(&theme.0) {
            return HResult(E_HANDLE);
        }
        HResult(S_OK)
    }

    pub fn draw_theme_edge(
        &self,
        theme: HTheme,
        _hdc: WinHandle,
        _part_id: u32,
        _state_id: u32,
        _dest_rect: &Rect,
        _edge: u32,
        _flags: u32,
    ) -> HResult {
        if !self.themes.contains_key(&theme.0) {
            return HResult(E_HANDLE);
        }
        HResult(S_OK)
    }

    pub fn draw_theme_parent_background(
        &self,
        _hwnd: WinHandle,
        _hdc: WinHandle,
        _rect: Option<&Rect>,
    ) -> HResult {
        HResult(S_OK)
    }

    pub fn draw_theme_parent_background_ex(
        &self,
        _hwnd: WinHandle,
        _hdc: WinHandle,
        _flags: u32,
        _rect: Option<&Rect>,
    ) -> HResult {
        HResult(S_OK)
    }

    // --- Theme metrics ---

    pub fn get_theme_background_content_rect(
        &self,
        theme: HTheme,
        _hdc: WinHandle,
        _part_id: u32,
        _state_id: u32,
        bounding_rect: &Rect,
    ) -> Result<Rect, HResult> {
        if !self.themes.contains_key(&theme.0) {
            return Err(HResult(E_HANDLE));
        }
        Ok(Rect {
            left: bounding_rect.left + 2,
            top: bounding_rect.top + 2,
            right: bounding_rect.right - 2,
            bottom: bounding_rect.bottom - 2,
        })
    }

    pub fn get_theme_background_extent(
        &self,
        theme: HTheme,
        _hdc: WinHandle,
        _part_id: u32,
        _state_id: u32,
        content_rect: &Rect,
    ) -> Result<Rect, HResult> {
        if !self.themes.contains_key(&theme.0) {
            return Err(HResult(E_HANDLE));
        }
        Ok(Rect {
            left: content_rect.left - 2,
            top: content_rect.top - 2,
            right: content_rect.right + 2,
            bottom: content_rect.bottom + 2,
        })
    }

    pub fn get_theme_part_size(
        &self,
        theme: HTheme,
        _hdc: WinHandle,
        part_id: u32,
        state_id: u32,
        _rect: Option<&Rect>,
        size_type: ThemeSizeType,
    ) -> Result<Size, HResult> {
        let data = self.themes.get(&theme.0).ok_or(HResult(E_HANDLE))?;
        if let Some(size) = data.sizes.get(&(part_id, state_id)) {
            return Ok(*size);
        }
        let s = match size_type {
            ThemeSizeType::Min => Size { cx: 8, cy: 8 },
            ThemeSizeType::True => Size { cx: 16, cy: 16 },
            ThemeSizeType::Draw => Size { cx: 24, cy: 24 },
        };
        Ok(s)
    }

    pub fn get_theme_text_extent(
        &self,
        theme: HTheme,
        _hdc: WinHandle,
        _part_id: u32,
        _state_id: u32,
        text: &str,
        _flags: u32,
        _bounding_rect: Option<&Rect>,
    ) -> Result<Rect, HResult> {
        if !self.themes.contains_key(&theme.0) {
            return Err(HResult(E_HANDLE));
        }
        let width = text.len() as i32 * 7;
        let height = 16;
        Ok(Rect {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        })
    }

    pub fn get_theme_margins(
        &self,
        theme: HTheme,
        _hdc: WinHandle,
        part_id: u32,
        state_id: u32,
        prop_id: u32,
    ) -> Result<ThemeMargins, HResult> {
        let data = self.themes.get(&theme.0).ok_or(HResult(E_HANDLE))?;
        if let Some(m) = data.margins_map.get(&(part_id, state_id, prop_id)) {
            return Ok(*m);
        }
        Ok(ThemeMargins {
            left: 2,
            right: 2,
            top: 2,
            bottom: 2,
        })
    }

    pub fn get_theme_metric(
        &self,
        theme: HTheme,
        _hdc: WinHandle,
        part_id: u32,
        state_id: u32,
        prop_id: u32,
    ) -> Result<i32, HResult> {
        let data = self.themes.get(&theme.0).ok_or(HResult(E_HANDLE))?;
        if let Some(&v) = data.integers.get(&(part_id, state_id, prop_id)) {
            return Ok(v);
        }
        Ok(0)
    }

    pub fn get_theme_bool(
        &self,
        theme: HTheme,
        part_id: u32,
        state_id: u32,
        prop_id: u32,
    ) -> Result<bool, HResult> {
        let data = self.themes.get(&theme.0).ok_or(HResult(E_HANDLE))?;
        if let Some(&v) = data.booleans.get(&(part_id, state_id, prop_id)) {
            return Ok(v);
        }
        Ok(false)
    }

    pub fn get_theme_color(
        &self,
        theme: HTheme,
        part_id: u32,
        state_id: u32,
        prop_id: u32,
    ) -> Result<u32, HResult> {
        let data = self.themes.get(&theme.0).ok_or(HResult(E_HANDLE))?;
        if let Some(&c) = data.colors.get(&(part_id, state_id, prop_id)) {
            return Ok(c);
        }
        Ok(0x00000000)
    }

    pub fn get_theme_enum_value(
        &self,
        theme: HTheme,
        part_id: u32,
        state_id: u32,
        prop_id: u32,
    ) -> Result<i32, HResult> {
        let data = self.themes.get(&theme.0).ok_or(HResult(E_HANDLE))?;
        if let Some(&v) = data.integers.get(&(part_id, state_id, prop_id)) {
            return Ok(v);
        }
        Ok(0)
    }

    pub fn get_theme_filename(
        &self,
        theme: HTheme,
        _part_id: u32,
        _state_id: u32,
        _prop_id: u32,
    ) -> Result<String, HResult> {
        if !self.themes.contains_key(&theme.0) {
            return Err(HResult(E_HANDLE));
        }
        Ok(String::new())
    }

    pub fn get_theme_font(
        &self,
        theme: HTheme,
        _hdc: WinHandle,
        part_id: u32,
        state_id: u32,
        prop_id: u32,
    ) -> Result<ThemeFont, HResult> {
        let data = self.themes.get(&theme.0).ok_or(HResult(E_HANDLE))?;
        if let Some(f) = data.fonts.get(&(part_id, state_id, prop_id)) {
            return Ok(f.clone());
        }
        Ok(ThemeFont {
            face_name: String::from("Segoe UI"),
            height: -12,
            width: 0,
            weight: 400,
            italic: false,
            underline: false,
            strikeout: false,
            char_set: 1,
            quality: 5,
        })
    }

    pub fn get_theme_int(
        &self,
        theme: HTheme,
        part_id: u32,
        state_id: u32,
        prop_id: u32,
    ) -> Result<i32, HResult> {
        self.get_theme_metric(theme, WinHandle(0), part_id, state_id, prop_id)
    }

    pub fn get_theme_int_list(
        &self,
        theme: HTheme,
        _part_id: u32,
        _state_id: u32,
        _prop_id: u32,
    ) -> Result<ThemeIntList, HResult> {
        if !self.themes.contains_key(&theme.0) {
            return Err(HResult(E_HANDLE));
        }
        Ok(ThemeIntList { values: Vec::new() })
    }

    pub fn get_theme_position(
        &self,
        theme: HTheme,
        _part_id: u32,
        _state_id: u32,
        _prop_id: u32,
    ) -> Result<(i32, i32), HResult> {
        if !self.themes.contains_key(&theme.0) {
            return Err(HResult(E_HANDLE));
        }
        Ok((0, 0))
    }

    pub fn get_theme_rect(
        &self,
        theme: HTheme,
        _part_id: u32,
        _state_id: u32,
        _prop_id: u32,
    ) -> Result<Rect, HResult> {
        if !self.themes.contains_key(&theme.0) {
            return Err(HResult(E_HANDLE));
        }
        Ok(Rect::default())
    }

    pub fn get_theme_string(
        &self,
        theme: HTheme,
        _part_id: u32,
        _state_id: u32,
        _prop_id: u32,
    ) -> Result<String, HResult> {
        if !self.themes.contains_key(&theme.0) {
            return Err(HResult(E_HANDLE));
        }
        Ok(String::new())
    }

    pub fn get_theme_sys_color(&self, _theme: HTheme, color_id: i32) -> u32 {
        self.sys_colors.get(&color_id).copied().unwrap_or(0)
    }

    pub fn get_theme_sys_color_brush(&self, _theme: HTheme, _color_id: i32) -> WinHandle {
        WinHandle(0)
    }

    pub fn get_theme_sys_font(&self, _theme: HTheme, font_id: i32) -> ThemeFont {
        self.sys_fonts.get(&font_id).cloned().unwrap_or(ThemeFont {
            face_name: String::from("Segoe UI"),
            height: -12,
            width: 0,
            weight: 400,
            italic: false,
            underline: false,
            strikeout: false,
            char_set: 1,
            quality: 5,
        })
    }

    pub fn get_theme_sys_int(&self, _theme: HTheme, _int_id: i32) -> Result<i32, HResult> {
        Ok(0)
    }

    pub fn get_theme_sys_size(&self, _theme: HTheme, size_id: i32) -> i32 {
        self.sys_metrics.get(&size_id).copied().unwrap_or(0)
    }

    pub fn get_theme_sys_string(&self, _theme: HTheme, _string_id: i32) -> Result<String, HResult> {
        Ok(String::new())
    }

    pub fn get_theme_transition_duration(
        &self,
        theme: HTheme,
        part_id: u32,
        state_from: u32,
        state_to: u32,
    ) -> Result<u32, HResult> {
        let data = self.themes.get(&theme.0).ok_or(HResult(E_HANDLE))?;
        if let Some(&d) = data
            .transition_durations
            .get(&(part_id, state_from, state_to))
        {
            return Ok(d);
        }
        Ok(200)
    }

    // --- State checking ---

    pub fn is_theme_active(&self) -> bool {
        self.theme_active
    }

    pub fn is_app_themed(&self) -> bool {
        self.app_themed
    }

    pub fn is_theme_part_defined(&self, theme: HTheme, _part_id: u32, _state_id: u32) -> bool {
        self.themes.contains_key(&theme.0)
    }

    pub fn is_theme_background_partially_transparent(
        &self,
        theme: HTheme,
        _part_id: u32,
        _state_id: u32,
    ) -> bool {
        self.themes.contains_key(&theme.0)
    }

    pub fn is_composition_active(&self) -> bool {
        self.composition_active
    }

    // --- DPI ---

    pub fn get_theme_sys_dpi(&self) -> u32 {
        self.dpi
    }

    pub fn get_theme_dpi_scaled_pixels(&self, _theme: HTheme, pixels: i32) -> i32 {
        (pixels as u64 * self.dpi as u64 / 96) as i32
    }

    // --- Buffered paint ---

    pub fn buffered_paint_init(&mut self) -> HResult {
        self.buffered_paint_init = true;
        HResult(S_OK)
    }

    pub fn buffered_paint_uninit(&mut self) -> HResult {
        self.buffered_paint_init = false;
        self.buffered_paints.clear();
        HResult(S_OK)
    }

    pub fn begin_buffered_paint(
        &mut self,
        hdc_target: WinHandle,
        rc_target: &Rect,
        _format: BufferedPaintFormat,
        _params: Option<&BufferedPaintParams>,
    ) -> HBufferedPaint {
        let id = self.next_bp_handle;
        self.next_bp_handle += 1;
        let handle = HBufferedPaint(id);
        let width = (rc_target.right - rc_target.left).max(1) as u32;
        let height = (rc_target.bottom - rc_target.top).max(1) as u32;
        self.buffered_paints.insert(
            id,
            BufferedPaintState {
                handle,
                hdc_target,
                hdc_buffer: WinHandle(id + 0x80000000),
                rc_target: *rc_target,
                bits: Vec::new(),
                width,
                height,
            },
        );
        handle
    }

    pub fn end_buffered_paint(&mut self, handle: HBufferedPaint, update_target: bool) -> HResult {
        let _ = update_target;
        if self.buffered_paints.remove(&handle.0).is_some() {
            HResult(S_OK)
        } else {
            HResult(E_HANDLE)
        }
    }

    pub fn get_buffered_paint_bits(&self, handle: HBufferedPaint) -> Option<&[u8]> {
        self.buffered_paints
            .get(&handle.0)
            .map(|bp| bp.bits.as_slice())
    }

    pub fn get_buffered_paint_dc(&self, handle: HBufferedPaint) -> Option<WinHandle> {
        self.buffered_paints.get(&handle.0).map(|bp| bp.hdc_buffer)
    }

    pub fn begin_buffered_animation(
        &mut self,
        _hwnd: WinHandle,
        _hdc_target: WinHandle,
        _rc_target: &Rect,
        _format: BufferedPaintFormat,
        _params: Option<&BufferedPaintParams>,
        _duration: u32,
    ) -> HBufferedAnimation {
        HBufferedAnimation(0)
    }

    pub fn end_buffered_animation(
        &mut self,
        _handle: HBufferedAnimation,
        _update: bool,
    ) -> HResult {
        HResult(S_OK)
    }

    pub fn buffered_paint_set_alpha(
        &mut self,
        handle: HBufferedPaint,
        _rect: Option<&Rect>,
        alpha: u8,
    ) -> HResult {
        if let Some(bp) = self.buffered_paints.get_mut(&handle.0) {
            let pixel_count = (bp.width * bp.height) as usize;
            if bp.bits.len() >= pixel_count * 4 {
                for i in 0..pixel_count {
                    bp.bits[i * 4 + 3] = alpha;
                }
            }
            HResult(S_OK)
        } else {
            HResult(E_HANDLE)
        }
    }

    // --- SetWindowTheme ---

    pub fn set_window_theme(
        &mut self,
        hwnd: WinHandle,
        sub_app_name: Option<&str>,
        _sub_id_list: Option<&str>,
    ) -> HResult {
        if let Some(name) = sub_app_name {
            self.window_themes.insert(hwnd.0, String::from(name));
        } else {
            self.window_themes.remove(&hwnd.0);
        }
        HResult(S_OK)
    }
}

// ---------------------------------------------------------------------------
// Draw theme options
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct DrawThemeBackgroundOptions {
    pub flags: u32,
    pub clip_rect: Option<Rect>,
}

#[derive(Debug, Clone)]
pub struct DrawThemeTextOptions {
    pub flags: u32,
    pub color: u32,
    pub color_specified: bool,
    pub border_size: i32,
    pub border_color: u32,
    pub shadow_type: i32,
    pub shadow_color: u32,
    pub shadow_offset_x: i32,
    pub shadow_offset_y: i32,
    pub glow_size: i32,
    pub apply_overlay: bool,
    pub callback_params: u64,
}

// ---------------------------------------------------------------------------
// Default theme population
// ---------------------------------------------------------------------------

fn populate_theme_defaults(data: &mut ThemeData) {
    let class = data.class_name.as_str();
    match class {
        "BUTTON" | "Button" => {
            data.colors.insert((1, 1, 3803), 0x00000000); // text color normal
            data.colors.insert((1, 2, 3803), 0x00000000); // text color hot
            data.colors.insert((1, 3, 3803), 0x00000000); // text color pressed
            data.colors.insert((1, 4, 3803), 0x006D6D6D); // text color disabled
            data.sizes.insert((1, 1), Size { cx: 75, cy: 23 });
        }
        "EDIT" | "Edit" => {
            data.colors.insert((1, 1, 3803), 0x00000000);
            data.colors.insert((3, 1, 3822), 0x00C0C0C0);
        }
        "SCROLLBAR" | "Scrollbar" => {
            data.sizes.insert((1, 1), Size { cx: 17, cy: 17 });
        }
        "WINDOW" | "Window" => {
            data.sizes.insert((1, 1), Size { cx: 0, cy: 30 });
            data.colors.insert((1, 1, 3803), 0x00000000);
        }
        "PROGRESS" | "Progress" => {
            data.colors.insert((5, 1, 3802), 0x0006B025);
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Global UxTheme runtime
// ---------------------------------------------------------------------------

static mut UXTHEME: UxThemeApi = UxThemeApi::new();

pub fn init() {
    unsafe {
        UXTHEME.init();
    }
}

pub fn api() -> &'static UxThemeApi {
    unsafe { &UXTHEME }
}

pub fn api_mut() -> &'static mut UxThemeApi {
    unsafe { &mut UXTHEME }
}
