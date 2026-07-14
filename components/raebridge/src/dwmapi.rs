//! Desktop Window Manager (DWM) API emulation for RaeBridge.
#![no_std]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::{HResult, Rect, WinBool, WinHandle};

// ---------------------------------------------------------------------------
// HRESULT constants
// ---------------------------------------------------------------------------

pub const S_OK: i32 = 0;
pub const S_FALSE: i32 = 1;
pub const E_INVALIDARG: i32 = -2147024809;
pub const DWM_E_COMPOSITIONDISABLED: i32 = -2003302336_i32;

// ---------------------------------------------------------------------------
// MARGINS
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default)]
pub struct DwmMargins {
    pub left: i32,
    pub right: i32,
    pub top: i32,
    pub bottom: i32,
}

impl DwmMargins {
    pub fn all(value: i32) -> Self {
        Self {
            left: value,
            right: value,
            top: value,
            bottom: value,
        }
    }

    pub fn extend_full() -> Self {
        Self {
            left: -1,
            right: -1,
            top: -1,
            bottom: -1,
        }
    }

    pub fn is_full_glass(&self) -> bool {
        self.left == -1 && self.right == -1 && self.top == -1 && self.bottom == -1
    }
}

// ---------------------------------------------------------------------------
// DWM window attributes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum DwmWindowAttribute {
    NcRenderingEnabled = 1,
    NcRenderingPolicy = 2,
    TransitionsForceDisabled = 3,
    AllowNcPaint = 4,
    CaptionButtonBounds = 5,
    NonclientRtlLayout = 6,
    ForceIconicRepresentation = 7,
    Flip3DPolicy = 8,
    ExtendedFrameBounds = 9,
    HasIconicBitmap = 10,
    DisallowPeek = 11,
    ExcludedFromPeek = 12,
    Cloak = 13,
    Cloaked = 14,
    FreezeRepresentation = 15,
    PassiveUpdateMode = 16,
    UseHostBackdropBrush = 17,
    UseImmersiveDarkMode = 20,
    WindowCornerPreference = 33,
    BorderColor = 34,
    CaptionColor = 35,
    TextColor = 36,
    VisibleFrameBorderThickness = 37,
    SystemBackdropType = 38,
    MicaEffect = 1029,
}

// ---------------------------------------------------------------------------
// NC rendering policy
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum DwmNcRenderingPolicy {
    UseWindowStyle = 0,
    Disabled = 1,
    Enabled = 2,
}

// ---------------------------------------------------------------------------
// Flip3D policy
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum DwmFlip3DPolicy {
    Default = 0,
    ExcludeBelow = 1,
    ExcludeAbove = 2,
}

// ---------------------------------------------------------------------------
// Backdrop types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum DwmSystemBackdropType {
    Auto = 0,
    None = 1,
    MainWindow = 2,
    TransientWindow = 3,
    TabbedWindow = 4,
}

// ---------------------------------------------------------------------------
// Window corner preferences
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum DwmWindowCornerPreference {
    Default = 0,
    DoNotRound = 1,
    Round = 2,
    RoundSmall = 3,
}

// ---------------------------------------------------------------------------
// Blur behind
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct DwmBlurBehind {
    pub flags: u32,
    pub enable: bool,
    pub region_handle: Option<WinHandle>,
    pub transition_on_maximized: bool,
}

impl DwmBlurBehind {
    pub const DWM_BB_ENABLE: u32 = 0x1;
    pub const DWM_BB_BLURREGION: u32 = 0x2;
    pub const DWM_BB_TRANSITIONONMAXIMIZED: u32 = 0x4;
}

// ---------------------------------------------------------------------------
// Thumbnail properties
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct DwmThumbnailProperties {
    pub flags: u32,
    pub rc_destination: Rect,
    pub rc_source: Rect,
    pub opacity: u8,
    pub visible: bool,
    pub source_client_area_only: bool,
}

impl DwmThumbnailProperties {
    pub const DWM_TNP_RECTDESTINATION: u32 = 0x1;
    pub const DWM_TNP_RECTSOURCE: u32 = 0x2;
    pub const DWM_TNP_OPACITY: u32 = 0x4;
    pub const DWM_TNP_VISIBLE: u32 = 0x8;
    pub const DWM_TNP_SOURCECLIENTAREAONLY: u32 = 0x10;
}

// ---------------------------------------------------------------------------
// Thumbnail handle
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DwmThumbnailHandle(pub u64);

// ---------------------------------------------------------------------------
// Colorization params
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct DwmColorizationParams {
    pub color: u32,
    pub afterglow: u32,
    pub color_balance: u32,
    pub afterglow_balance: u32,
    pub blur_balance: u32,
    pub glass_reflection_intensity: u32,
    pub opaque_blend: bool,
}

// ---------------------------------------------------------------------------
// Per-window attribute state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct DwmWindowState {
    pub hwnd: WinHandle,
    pub nc_rendering_policy: DwmNcRenderingPolicy,
    pub transitions_disabled: bool,
    pub allow_nc_paint: bool,
    pub force_iconic_representation: bool,
    pub flip3d_policy: DwmFlip3DPolicy,
    pub has_iconic_bitmap: bool,
    pub disallow_peek: bool,
    pub excluded_from_peek: bool,
    pub cloaked: bool,
    pub freeze_representation: bool,
    pub passive_update_mode: bool,
    pub use_host_backdrop_brush: bool,
    pub use_immersive_dark_mode: bool,
    pub corner_preference: DwmWindowCornerPreference,
    pub border_color: u32,
    pub caption_color: u32,
    pub text_color: u32,
    pub system_backdrop_type: DwmSystemBackdropType,
    pub mica_effect: bool,
    pub frame_margins: DwmMargins,
    pub blur_behind: Option<DwmBlurBehind>,
}

impl DwmWindowState {
    pub fn new(hwnd: WinHandle) -> Self {
        Self {
            hwnd,
            nc_rendering_policy: DwmNcRenderingPolicy::UseWindowStyle,
            transitions_disabled: false,
            allow_nc_paint: true,
            force_iconic_representation: false,
            flip3d_policy: DwmFlip3DPolicy::Default,
            has_iconic_bitmap: false,
            disallow_peek: false,
            excluded_from_peek: false,
            cloaked: false,
            freeze_representation: false,
            passive_update_mode: false,
            use_host_backdrop_brush: false,
            use_immersive_dark_mode: false,
            corner_preference: DwmWindowCornerPreference::Default,
            border_color: 0xFFFFFFFF,
            caption_color: 0xFFFFFFFF,
            text_color: 0xFF000000,
            system_backdrop_type: DwmSystemBackdropType::Auto,
            mica_effect: false,
            frame_margins: DwmMargins::default(),
            blur_behind: None,
        }
    }
}

// ---------------------------------------------------------------------------
// DWM API functions
// ---------------------------------------------------------------------------

pub struct DwmApi {
    pub initialized: bool,
    pub composition_enabled: bool,
    pub window_states: BTreeMap<u64, DwmWindowState>,
    pub thumbnails: BTreeMap<u64, DwmThumbnailRegistration>,
    pub next_thumbnail: u64,
    pub colorization_color: u32,
    pub colorization_opaque_blend: bool,
    pub mmcss_enabled: bool,
    pub accent_color: u32,
    pub frame_count: u64,
}

#[derive(Debug, Clone)]
pub struct DwmThumbnailRegistration {
    pub handle: DwmThumbnailHandle,
    pub source_hwnd: WinHandle,
    pub dest_hwnd: WinHandle,
    pub properties: DwmThumbnailProperties,
}

impl DwmApi {
    pub const fn new() -> Self {
        Self {
            initialized: false,
            composition_enabled: true,
            window_states: BTreeMap::new(),
            thumbnails: BTreeMap::new(),
            next_thumbnail: 0xDC_000001,
            colorization_color: 0xC40078D7,
            colorization_opaque_blend: true,
            mmcss_enabled: false,
            accent_color: 0xFF0078D7,
            frame_count: 0,
        }
    }

    pub fn init(&mut self) {
        if self.initialized {
            return;
        }
        self.initialized = true;
    }

    pub fn extend_frame_into_client_area(
        &mut self,
        hwnd: WinHandle,
        margins: &DwmMargins,
    ) -> HResult {
        let state = self
            .window_states
            .entry(hwnd.0)
            .or_insert_with(|| DwmWindowState::new(hwnd));
        state.frame_margins = *margins;
        HResult(S_OK)
    }

    pub fn set_window_attribute(
        &mut self,
        hwnd: WinHandle,
        attribute: DwmWindowAttribute,
        value: &[u8],
    ) -> HResult {
        let state = self
            .window_states
            .entry(hwnd.0)
            .or_insert_with(|| DwmWindowState::new(hwnd));

        match attribute {
            DwmWindowAttribute::NcRenderingPolicy => {
                if value.len() >= 4 {
                    let v = u32::from_le_bytes([value[0], value[1], value[2], value[3]]);
                    state.nc_rendering_policy = match v {
                        0 => DwmNcRenderingPolicy::UseWindowStyle,
                        1 => DwmNcRenderingPolicy::Disabled,
                        _ => DwmNcRenderingPolicy::Enabled,
                    };
                }
            }
            DwmWindowAttribute::TransitionsForceDisabled => {
                if value.len() >= 4 {
                    let v = u32::from_le_bytes([value[0], value[1], value[2], value[3]]);
                    state.transitions_disabled = v != 0;
                }
            }
            DwmWindowAttribute::AllowNcPaint => {
                if value.len() >= 4 {
                    let v = u32::from_le_bytes([value[0], value[1], value[2], value[3]]);
                    state.allow_nc_paint = v != 0;
                }
            }
            DwmWindowAttribute::ForceIconicRepresentation => {
                if value.len() >= 4 {
                    let v = u32::from_le_bytes([value[0], value[1], value[2], value[3]]);
                    state.force_iconic_representation = v != 0;
                }
            }
            DwmWindowAttribute::Flip3DPolicy => {
                if value.len() >= 4 {
                    let v = u32::from_le_bytes([value[0], value[1], value[2], value[3]]);
                    state.flip3d_policy = match v {
                        1 => DwmFlip3DPolicy::ExcludeBelow,
                        2 => DwmFlip3DPolicy::ExcludeAbove,
                        _ => DwmFlip3DPolicy::Default,
                    };
                }
            }
            DwmWindowAttribute::HasIconicBitmap => {
                if value.len() >= 4 {
                    let v = u32::from_le_bytes([value[0], value[1], value[2], value[3]]);
                    state.has_iconic_bitmap = v != 0;
                }
            }
            DwmWindowAttribute::DisallowPeek => {
                if value.len() >= 4 {
                    let v = u32::from_le_bytes([value[0], value[1], value[2], value[3]]);
                    state.disallow_peek = v != 0;
                }
            }
            DwmWindowAttribute::ExcludedFromPeek => {
                if value.len() >= 4 {
                    let v = u32::from_le_bytes([value[0], value[1], value[2], value[3]]);
                    state.excluded_from_peek = v != 0;
                }
            }
            DwmWindowAttribute::Cloak => {
                if value.len() >= 4 {
                    let v = u32::from_le_bytes([value[0], value[1], value[2], value[3]]);
                    state.cloaked = v != 0;
                }
            }
            DwmWindowAttribute::FreezeRepresentation => {
                if value.len() >= 4 {
                    let v = u32::from_le_bytes([value[0], value[1], value[2], value[3]]);
                    state.freeze_representation = v != 0;
                }
            }
            DwmWindowAttribute::PassiveUpdateMode => {
                if value.len() >= 4 {
                    let v = u32::from_le_bytes([value[0], value[1], value[2], value[3]]);
                    state.passive_update_mode = v != 0;
                }
            }
            DwmWindowAttribute::UseHostBackdropBrush => {
                if value.len() >= 4 {
                    let v = u32::from_le_bytes([value[0], value[1], value[2], value[3]]);
                    state.use_host_backdrop_brush = v != 0;
                }
            }
            DwmWindowAttribute::UseImmersiveDarkMode => {
                if value.len() >= 4 {
                    let v = u32::from_le_bytes([value[0], value[1], value[2], value[3]]);
                    state.use_immersive_dark_mode = v != 0;
                }
            }
            DwmWindowAttribute::WindowCornerPreference => {
                if value.len() >= 4 {
                    let v = u32::from_le_bytes([value[0], value[1], value[2], value[3]]);
                    state.corner_preference = match v {
                        1 => DwmWindowCornerPreference::DoNotRound,
                        2 => DwmWindowCornerPreference::Round,
                        3 => DwmWindowCornerPreference::RoundSmall,
                        _ => DwmWindowCornerPreference::Default,
                    };
                }
            }
            DwmWindowAttribute::BorderColor => {
                if value.len() >= 4 {
                    state.border_color =
                        u32::from_le_bytes([value[0], value[1], value[2], value[3]]);
                }
            }
            DwmWindowAttribute::CaptionColor => {
                if value.len() >= 4 {
                    state.caption_color =
                        u32::from_le_bytes([value[0], value[1], value[2], value[3]]);
                }
            }
            DwmWindowAttribute::TextColor => {
                if value.len() >= 4 {
                    state.text_color = u32::from_le_bytes([value[0], value[1], value[2], value[3]]);
                }
            }
            DwmWindowAttribute::SystemBackdropType => {
                if value.len() >= 4 {
                    let v = u32::from_le_bytes([value[0], value[1], value[2], value[3]]);
                    state.system_backdrop_type = match v {
                        1 => DwmSystemBackdropType::None,
                        2 => DwmSystemBackdropType::MainWindow,
                        3 => DwmSystemBackdropType::TransientWindow,
                        4 => DwmSystemBackdropType::TabbedWindow,
                        _ => DwmSystemBackdropType::Auto,
                    };
                }
            }
            DwmWindowAttribute::MicaEffect => {
                if value.len() >= 4 {
                    let v = u32::from_le_bytes([value[0], value[1], value[2], value[3]]);
                    state.mica_effect = v != 0;
                }
            }
            _ => {}
        }
        HResult(S_OK)
    }

    pub fn get_window_attribute(
        &self,
        hwnd: WinHandle,
        attribute: DwmWindowAttribute,
        out: &mut [u8],
    ) -> HResult {
        let state = match self.window_states.get(&hwnd.0) {
            Some(s) => s,
            None => return HResult(E_INVALIDARG),
        };

        if out.len() < 4 {
            return HResult(E_INVALIDARG);
        }

        let value: u32 = match attribute {
            DwmWindowAttribute::NcRenderingEnabled => 1,
            DwmWindowAttribute::NcRenderingPolicy => state.nc_rendering_policy as u32,
            DwmWindowAttribute::TransitionsForceDisabled => state.transitions_disabled as u32,
            DwmWindowAttribute::AllowNcPaint => state.allow_nc_paint as u32,
            DwmWindowAttribute::ForceIconicRepresentation => {
                state.force_iconic_representation as u32
            }
            DwmWindowAttribute::Flip3DPolicy => state.flip3d_policy as u32,
            DwmWindowAttribute::HasIconicBitmap => state.has_iconic_bitmap as u32,
            DwmWindowAttribute::DisallowPeek => state.disallow_peek as u32,
            DwmWindowAttribute::ExcludedFromPeek => state.excluded_from_peek as u32,
            DwmWindowAttribute::Cloaked => state.cloaked as u32,
            DwmWindowAttribute::FreezeRepresentation => state.freeze_representation as u32,
            DwmWindowAttribute::UseImmersiveDarkMode => state.use_immersive_dark_mode as u32,
            DwmWindowAttribute::WindowCornerPreference => state.corner_preference as u32,
            DwmWindowAttribute::BorderColor => state.border_color,
            DwmWindowAttribute::CaptionColor => state.caption_color,
            DwmWindowAttribute::TextColor => state.text_color,
            DwmWindowAttribute::SystemBackdropType => state.system_backdrop_type as u32,
            DwmWindowAttribute::MicaEffect => state.mica_effect as u32,
            DwmWindowAttribute::VisibleFrameBorderThickness => 1,
            _ => 0,
        };

        out[..4].copy_from_slice(&value.to_le_bytes());
        HResult(S_OK)
    }

    pub fn is_composition_enabled(&self) -> WinBool {
        WinBool::from_bool(self.composition_enabled)
    }

    pub fn enable_blur_behind_window(
        &mut self,
        hwnd: WinHandle,
        blur_behind: &DwmBlurBehind,
    ) -> HResult {
        let state = self
            .window_states
            .entry(hwnd.0)
            .or_insert_with(|| DwmWindowState::new(hwnd));
        state.blur_behind = Some(blur_behind.clone());
        HResult(S_OK)
    }

    pub fn set_iconic_thumbnail(
        &mut self,
        hwnd: WinHandle,
        _bitmap: WinHandle,
        _max_width: u32,
        _max_height: u32,
    ) -> HResult {
        let state = self
            .window_states
            .entry(hwnd.0)
            .or_insert_with(|| DwmWindowState::new(hwnd));
        state.has_iconic_bitmap = true;
        HResult(S_OK)
    }

    pub fn set_iconic_live_preview_bitmap(
        &mut self,
        hwnd: WinHandle,
        _bitmap: WinHandle,
        _offset_x: i32,
        _offset_y: i32,
        _flags: u32,
    ) -> HResult {
        let _state = self
            .window_states
            .entry(hwnd.0)
            .or_insert_with(|| DwmWindowState::new(hwnd));
        HResult(S_OK)
    }

    pub fn invalidate_iconic_bitmaps(&mut self, hwnd: WinHandle) -> HResult {
        if self.window_states.contains_key(&hwnd.0) {
            HResult(S_OK)
        } else {
            HResult(E_INVALIDARG)
        }
    }

    pub fn register_thumbnail(
        &mut self,
        dest_hwnd: WinHandle,
        source_hwnd: WinHandle,
    ) -> Result<DwmThumbnailHandle, HResult> {
        let id = self.next_thumbnail;
        self.next_thumbnail += 1;
        let handle = DwmThumbnailHandle(id);
        self.thumbnails.insert(
            id,
            DwmThumbnailRegistration {
                handle,
                source_hwnd,
                dest_hwnd,
                properties: DwmThumbnailProperties {
                    flags: 0,
                    rc_destination: Rect::default(),
                    rc_source: Rect::default(),
                    opacity: 255,
                    visible: true,
                    source_client_area_only: false,
                },
            },
        );
        Ok(handle)
    }

    pub fn unregister_thumbnail(&mut self, thumbnail: DwmThumbnailHandle) -> HResult {
        if self.thumbnails.remove(&thumbnail.0).is_some() {
            HResult(S_OK)
        } else {
            HResult(E_INVALIDARG)
        }
    }

    pub fn update_thumbnail_properties(
        &mut self,
        thumbnail: DwmThumbnailHandle,
        properties: &DwmThumbnailProperties,
    ) -> HResult {
        match self.thumbnails.get_mut(&thumbnail.0) {
            Some(reg) => {
                reg.properties = *properties;
                HResult(S_OK)
            }
            None => HResult(E_INVALIDARG),
        }
    }

    pub fn get_colorization_color(&self) -> (u32, bool) {
        (self.colorization_color, self.colorization_opaque_blend)
    }

    pub fn flush(&mut self) -> HResult {
        self.frame_count += 1;
        HResult(S_OK)
    }

    pub fn def_window_proc(
        &self,
        _hwnd: WinHandle,
        _msg: u32,
        _wparam: u64,
        _lparam: i64,
    ) -> Option<i64> {
        None
    }

    pub fn enable_mmcss(&mut self, enable: bool) -> HResult {
        self.mmcss_enabled = enable;
        HResult(S_OK)
    }

    pub fn get_colorization_params(&self) -> DwmColorizationParams {
        DwmColorizationParams {
            color: self.colorization_color,
            afterglow: self.colorization_color,
            color_balance: 8,
            afterglow_balance: 43,
            blur_balance: 49,
            glass_reflection_intensity: 50,
            opaque_blend: self.colorization_opaque_blend,
        }
    }
}

// ---------------------------------------------------------------------------
// Global DWMAPI runtime
// ---------------------------------------------------------------------------

static mut DWMAPI: DwmApi = DwmApi::new();

pub fn init() {
    unsafe {
        DWMAPI.init();
    }
}

pub fn api() -> &'static DwmApi {
    unsafe { &DWMAPI }
}

pub fn api_mut() -> &'static mut DwmApi {
    unsafe { &mut DWMAPI }
}
