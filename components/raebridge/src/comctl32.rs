//! comctl32.dll — Windows common controls: ListView, TreeView, ToolBar,
//! StatusBar, ProgressBar, TabControl, Tooltip, ImageList, PropertySheet,
//! UpDown, Header, and Rebar for RaeBridge.

use alloc::string::String;
use alloc::vec::Vec;

use crate::{CompatContext, ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER, ERROR_SUCCESS};

// =========================================================================
// Common control class flags (InitCommonControlsEx)
// =========================================================================

pub const ICC_LISTVIEW_CLASSES: u32 = 0x00000001;
pub const ICC_TREEVIEW_CLASSES: u32 = 0x00000002;
pub const ICC_BAR_CLASSES: u32 = 0x00000004;
pub const ICC_TAB_CLASSES: u32 = 0x00000008;
pub const ICC_UPDOWN_CLASS: u32 = 0x00000010;
pub const ICC_PROGRESS_CLASS: u32 = 0x00000020;
pub const ICC_HOTKEY_CLASS: u32 = 0x00000040;
pub const ICC_ANIMATE_CLASS: u32 = 0x00000080;
pub const ICC_DATE_CLASSES: u32 = 0x00000100;
pub const ICC_USEREX_CLASSES: u32 = 0x00000200;
pub const ICC_COOL_CLASSES: u32 = 0x00000400;
pub const ICC_INTERNET_CLASSES: u32 = 0x00000800;
pub const ICC_PAGESCROLLER_CLASS: u32 = 0x00001000;
pub const ICC_NATIVEFNTCTL_CLASS: u32 = 0x00002000;
pub const ICC_STANDARD_CLASSES: u32 = 0x00004000;
pub const ICC_LINK_CLASS: u32 = 0x00008000;

// =========================================================================
// ListView masks and styles
// =========================================================================

pub const LVIF_TEXT: u32 = 0x00000001;
pub const LVIF_IMAGE: u32 = 0x00000002;
pub const LVIF_PARAM: u32 = 0x00000004;
pub const LVIF_STATE: u32 = 0x00000008;
pub const LVIF_INDENT: u32 = 0x00000010;
pub const LVIF_GROUPID: u32 = 0x00000100;
pub const LVIF_COLUMNS: u32 = 0x00000200;

pub const LVIS_FOCUSED: u32 = 0x0001;
pub const LVIS_SELECTED: u32 = 0x0002;
pub const LVIS_CUT: u32 = 0x0004;
pub const LVIS_DROPHILITED: u32 = 0x0008;
pub const LVIS_OVERLAYMASK: u32 = 0x0F00;
pub const LVIS_STATEIMAGEMASK: u32 = 0xF000;

pub const LVCF_FMT: u32 = 0x0001;
pub const LVCF_WIDTH: u32 = 0x0002;
pub const LVCF_TEXT: u32 = 0x0004;
pub const LVCF_SUBITEM: u32 = 0x0008;
pub const LVCF_IMAGE: u32 = 0x0010;
pub const LVCF_ORDER: u32 = 0x0020;

pub const LVCFMT_LEFT: i32 = 0x0000;
pub const LVCFMT_RIGHT: i32 = 0x0001;
pub const LVCFMT_CENTER: i32 = 0x0002;

pub const LVNI_ALL: u32 = 0x0000;
pub const LVNI_FOCUSED: u32 = 0x0001;
pub const LVNI_SELECTED: u32 = 0x0002;
pub const LVNI_CUT: u32 = 0x0004;
pub const LVNI_DROPHILITED: u32 = 0x0008;

pub const LVHT_NOWHERE: u32 = 0x00000001;
pub const LVHT_ONITEMICON: u32 = 0x00000002;
pub const LVHT_ONITEMLABEL: u32 = 0x00000004;
pub const LVHT_ONITEM: u32 = 0x0000000E;

pub const LVSIL_NORMAL: i32 = 0;
pub const LVSIL_SMALL: i32 = 1;
pub const LVSIL_STATE: i32 = 2;

pub const LVSCW_AUTOSIZE: i32 = -1;
pub const LVSCW_AUTOSIZE_USEHEADER: i32 = -2;

// =========================================================================
// TreeView masks and styles
// =========================================================================

pub const TVIF_TEXT: u32 = 0x0001;
pub const TVIF_IMAGE: u32 = 0x0002;
pub const TVIF_PARAM: u32 = 0x0004;
pub const TVIF_STATE: u32 = 0x0008;
pub const TVIF_HANDLE: u32 = 0x0010;
pub const TVIF_SELECTEDIMAGE: u32 = 0x0020;
pub const TVIF_CHILDREN: u32 = 0x0040;

pub const TVIS_SELECTED: u32 = 0x0002;
pub const TVIS_CUT: u32 = 0x0004;
pub const TVIS_DROPHILITED: u32 = 0x0008;
pub const TVIS_BOLD: u32 = 0x0010;
pub const TVIS_EXPANDED: u32 = 0x0020;
pub const TVIS_EXPANDEDONCE: u32 = 0x0040;

pub const TVI_ROOT: u64 = 0xFFFF0000;
pub const TVI_FIRST: u64 = 0xFFFF0001;
pub const TVI_LAST: u64 = 0xFFFF0002;
pub const TVI_SORT: u64 = 0xFFFF0003;

pub const TVE_COLLAPSE: u32 = 0x0001;
pub const TVE_EXPAND: u32 = 0x0002;
pub const TVE_TOGGLE: u32 = 0x0003;

// =========================================================================
// ToolBar styles and constants
// =========================================================================

pub const TBSTATE_CHECKED: u8 = 0x01;
pub const TBSTATE_PRESSED: u8 = 0x02;
pub const TBSTATE_ENABLED: u8 = 0x04;
pub const TBSTATE_HIDDEN: u8 = 0x08;
pub const TBSTATE_INDETERMINATE: u8 = 0x10;
pub const TBSTATE_WRAP: u8 = 0x20;

pub const TBSTYLE_BUTTON: u32 = 0x0000;
pub const TBSTYLE_SEP: u32 = 0x0001;
pub const TBSTYLE_CHECK: u32 = 0x0002;
pub const TBSTYLE_GROUP: u32 = 0x0004;
pub const TBSTYLE_DROPDOWN: u32 = 0x0008;
pub const TBSTYLE_AUTOSIZE: u32 = 0x0010;
pub const TBSTYLE_FLAT: u32 = 0x0800;
pub const TBSTYLE_LIST: u32 = 0x1000;
pub const TBSTYLE_TOOLTIPS: u32 = 0x0100;

// =========================================================================
// StatusBar constants
// =========================================================================

pub const SB_SETTEXT: u32 = 0x0401;
pub const SB_GETTEXT: u32 = 0x0402;
pub const SB_GETTEXTLENGTH: u32 = 0x0403;
pub const SB_SETPARTS: u32 = 0x0404;
pub const SB_GETPARTS: u32 = 0x0406;
pub const SBT_OWNERDRAW: u32 = 0x1000;
pub const SBT_NOBORDERS: u32 = 0x0100;

// =========================================================================
// ProgressBar constants
// =========================================================================

pub const PBM_SETRANGE: u32 = 0x0401;
pub const PBM_SETPOS: u32 = 0x0402;
pub const PBM_DELTAPOS: u32 = 0x0403;
pub const PBM_SETSTEP: u32 = 0x0404;
pub const PBM_STEPIT: u32 = 0x0405;
pub const PBM_SETRANGE32: u32 = 0x0406;
pub const PBM_GETRANGE: u32 = 0x0407;
pub const PBM_GETPOS: u32 = 0x0408;
pub const PBM_SETMARQUEE: u32 = 0x040A;
pub const PBM_SETSTATE: u32 = 0x0410;

pub const PBS_SMOOTH: u32 = 0x01;
pub const PBS_VERTICAL: u32 = 0x04;
pub const PBS_MARQUEE: u32 = 0x08;

pub const PBST_NORMAL: u32 = 0x0001;
pub const PBST_ERROR: u32 = 0x0002;
pub const PBST_PAUSED: u32 = 0x0003;

// =========================================================================
// Tab Control constants
// =========================================================================

pub const TCM_INSERTITEM: u32 = 0x1307;
pub const TCM_DELETEITEM: u32 = 0x1308;
pub const TCM_DELETEALLITEMS: u32 = 0x1309;
pub const TCM_GETITEMCOUNT: u32 = 0x1304;
pub const TCM_GETCURSEL: u32 = 0x130B;
pub const TCM_SETCURSEL: u32 = 0x130C;
pub const TCM_ADJUSTRECT: u32 = 0x1328;
pub const TCM_GETITEM: u32 = 0x133C;
pub const TCM_SETITEM: u32 = 0x133D;

pub const TCIF_TEXT: u32 = 0x0001;
pub const TCIF_IMAGE: u32 = 0x0002;
pub const TCIF_PARAM: u32 = 0x0008;

// =========================================================================
// Tooltip constants
// =========================================================================

pub const TTM_ADDTOOL: u32 = 0x0432;
pub const TTM_DELTOOL: u32 = 0x0433;
pub const TTM_NEWTOOLRECT: u32 = 0x0434;
pub const TTM_GETTOOLINFO: u32 = 0x0435;
pub const TTM_SETTOOLINFO: u32 = 0x0436;
pub const TTM_GETTEXT: u32 = 0x0438;
pub const TTM_UPDATETIPTEXT: u32 = 0x0439;
pub const TTM_SETMAXTIPWIDTH: u32 = 0x0418;

pub const TTF_IDISHWND: u32 = 0x0001;
pub const TTF_CENTERTIP: u32 = 0x0002;
pub const TTF_SUBCLASS: u32 = 0x0010;
pub const TTF_TRACK: u32 = 0x0020;
pub const TTF_TRANSPARENT: u32 = 0x0100;

// =========================================================================
// ImageList constants
// =========================================================================

pub const ILC_COLOR: u32 = 0x00000000;
pub const ILC_COLOR4: u32 = 0x00000004;
pub const ILC_COLOR8: u32 = 0x00000008;
pub const ILC_COLOR16: u32 = 0x00000010;
pub const ILC_COLOR24: u32 = 0x00000018;
pub const ILC_COLOR32: u32 = 0x00000020;
pub const ILC_MASK: u32 = 0x00000001;

pub const ILD_NORMAL: u32 = 0x00000000;
pub const ILD_TRANSPARENT: u32 = 0x00000001;
pub const ILD_BLEND25: u32 = 0x00000002;
pub const ILD_BLEND50: u32 = 0x00000004;
pub const ILD_SELECTED: u32 = ILD_BLEND50;
pub const ILD_FOCUS: u32 = ILD_BLEND25;

// =========================================================================
// Header control constants
// =========================================================================

pub const HDM_INSERTITEM: u32 = 0x1200 + 10;
pub const HDM_DELETEITEM: u32 = 0x1200 + 2;
pub const HDM_GETITEMCOUNT: u32 = 0x1200 + 0;
pub const HDM_GETITEM: u32 = 0x1200 + 11;
pub const HDM_SETITEM: u32 = 0x1200 + 12;
pub const HDM_LAYOUT: u32 = 0x1200 + 5;

pub const HDF_LEFT: i32 = 0x0000;
pub const HDF_RIGHT: i32 = 0x0001;
pub const HDF_CENTER: i32 = 0x0002;
pub const HDF_STRING: i32 = 0x4000;
pub const HDF_SORTDOWN: i32 = 0x0200;
pub const HDF_SORTUP: i32 = 0x0400;

// =========================================================================
// Rebar constants
// =========================================================================

pub const RBIM_IMAGELIST: u32 = 0x00000001;
pub const RBBIM_STYLE: u32 = 0x00000001;
pub const RBBIM_COLORS: u32 = 0x00000002;
pub const RBBIM_TEXT: u32 = 0x00000004;
pub const RBBIM_IMAGE: u32 = 0x00000008;
pub const RBBIM_CHILD: u32 = 0x00000010;
pub const RBBIM_CHILDSIZE: u32 = 0x00000020;
pub const RBBIM_SIZE: u32 = 0x00000040;
pub const RBBIM_ID: u32 = 0x00000100;

pub const RBBS_BREAK: u32 = 0x00000001;
pub const RBBS_FIXEDSIZE: u32 = 0x00000002;
pub const RBBS_CHILDEDGE: u32 = 0x00000004;
pub const RBBS_HIDDEN: u32 = 0x00000008;
pub const RBBS_GRIPPERALWAYS: u32 = 0x00000080;
pub const RBBS_NOGRIPPER: u32 = 0x00000100;

// =========================================================================
// UpDown control constants
// =========================================================================

pub const UDM_SETRANGE: u32 = 0x0465;
pub const UDM_GETRANGE: u32 = 0x0466;
pub const UDM_SETPOS: u32 = 0x0467;
pub const UDM_GETPOS: u32 = 0x0468;
pub const UDM_SETRANGE32: u32 = 0x046F;
pub const UDM_GETRANGE32: u32 = 0x0470;
pub const UDM_SETPOS32: u32 = 0x0471;
pub const UDM_GETPOS32: u32 = 0x0472;

pub const UDS_WRAP: u32 = 0x0001;
pub const UDS_SETBUDDYINT: u32 = 0x0002;
pub const UDS_ALIGNRIGHT: u32 = 0x0004;
pub const UDS_ALIGNLEFT: u32 = 0x0008;
pub const UDS_AUTOBUDDY: u32 = 0x0010;
pub const UDS_ARROWKEYS: u32 = 0x0020;
pub const UDS_HORZ: u32 = 0x0040;
pub const UDS_NOTHOUSANDS: u32 = 0x0080;
pub const UDS_HOTTRACK: u32 = 0x0100;

// =========================================================================
// PropertySheet constants
// =========================================================================

pub const PSH_DEFAULT: u32 = 0x00000000;
pub const PSH_WIZARD: u32 = 0x00000020;
pub const PSH_PROPSHEETPAGE: u32 = 0x00000008;
pub const PSH_USECALLBACK: u32 = 0x00000100;
pub const PSH_HEADER: u32 = 0x00080000;
pub const PSH_WATERMARK: u32 = 0x00008000;

pub const PSP_DEFAULT: u32 = 0x00000000;
pub const PSP_DLGINDIRECT: u32 = 0x00000001;
pub const PSP_USEHICON: u32 = 0x00000002;
pub const PSP_USEICONID: u32 = 0x00000004;
pub const PSP_USETITLE: u32 = 0x00000008;
pub const PSP_USEREFPARENT: u32 = 0x00000040;
pub const PSP_PREMATURE: u32 = 0x00000400;
pub const PSP_HIDEHEADER: u32 = 0x00000800;
pub const PSP_USEHEADERTITLE: u32 = 0x00001000;
pub const PSP_USEHEADERSUBTITLE: u32 = 0x00002000;

// =========================================================================
// Data structures
// =========================================================================

#[derive(Debug, Clone)]
pub struct InitCommonControlsExData {
    pub size: u32,
    pub icc: u32,
}

#[derive(Debug, Clone)]
pub struct LvItem {
    pub mask: u32,
    pub item: i32,
    pub sub_item: i32,
    pub state: u32,
    pub state_mask: u32,
    pub text: String,
    pub image: i32,
    pub lparam: u64,
    pub indent: i32,
    pub group_id: i32,
    pub columns: u32,
}

impl LvItem {
    pub fn new() -> Self {
        Self {
            mask: 0,
            item: 0,
            sub_item: 0,
            state: 0,
            state_mask: 0,
            text: String::new(),
            image: 0,
            lparam: 0,
            indent: 0,
            group_id: -1,
            columns: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LvColumn {
    pub mask: u32,
    pub fmt: i32,
    pub cx: i32,
    pub text: String,
    pub sub_item: i32,
    pub image: i32,
    pub order: i32,
}

impl LvColumn {
    pub fn new() -> Self {
        Self {
            mask: 0,
            fmt: LVCFMT_LEFT,
            cx: 100,
            text: String::new(),
            sub_item: 0,
            image: 0,
            order: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LvHitTestInfo {
    pub pt: (i32, i32),
    pub flags: u32,
    pub item: i32,
    pub sub_item: i32,
    pub group: i32,
}

#[derive(Debug, Clone)]
pub struct TvItem {
    pub mask: u32,
    pub hitem: u64,
    pub state: u32,
    pub state_mask: u32,
    pub text: String,
    pub image: i32,
    pub selected_image: i32,
    pub children: i32,
    pub lparam: u64,
}

impl TvItem {
    pub fn new() -> Self {
        Self {
            mask: 0,
            hitem: 0,
            state: 0,
            state_mask: 0,
            text: String::new(),
            image: 0,
            selected_image: 0,
            children: 0,
            lparam: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TvInsertStruct {
    pub parent: u64,
    pub insert_after: u64,
    pub item: TvItem,
}

#[derive(Debug, Clone)]
pub struct TbButton {
    pub bitmap: i32,
    pub command: i32,
    pub state: u8,
    pub style: u32,
    pub data: u64,
    pub string_id: isize,
}

#[derive(Debug, Clone)]
pub struct TcItem {
    pub mask: u32,
    pub state: u32,
    pub state_mask: u32,
    pub text: String,
    pub image: i32,
    pub lparam: u64,
}

impl TcItem {
    pub fn new() -> Self {
        Self {
            mask: 0,
            state: 0,
            state_mask: 0,
            text: String::new(),
            image: -1,
            lparam: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToolInfo {
    pub size: u32,
    pub flags: u32,
    pub hwnd: u64,
    pub id: u64,
    pub rect: (i32, i32, i32, i32),
    pub inst: u64,
    pub text: String,
    pub lparam: u64,
}

#[derive(Debug, Clone)]
pub struct RebarBandInfo {
    pub size: u32,
    pub mask: u32,
    pub style: u32,
    pub clr_fore: u32,
    pub clr_back: u32,
    pub text: String,
    pub image: i32,
    pub child: u64,
    pub cx_min_child: u32,
    pub cy_min_child: u32,
    pub cx: u32,
    pub id: u32,
    pub cy_child: u32,
    pub cx_ideal: u32,
}

#[derive(Debug, Clone)]
pub struct HdItem {
    pub mask: u32,
    pub cxy: i32,
    pub text: String,
    pub fmt: i32,
    pub lparam: u64,
    pub image: i32,
    pub order: i32,
}

#[derive(Debug, Clone)]
pub struct PropertySheetPage {
    pub size: u32,
    pub flags: u32,
    pub inst: u64,
    pub title: String,
    pub dialog_proc: u64,
    pub lparam: u64,
}

#[derive(Debug, Clone)]
pub struct PropertySheetHeader {
    pub size: u32,
    pub flags: u32,
    pub hwnd_parent: u64,
    pub inst: u64,
    pub caption: String,
    pub pages: Vec<PropertySheetPage>,
    pub start_page: u32,
    pub callback: u64,
}

// =========================================================================
// Internal helpers
// =========================================================================

fn set_last_error(ctx: &mut CompatContext, code: u32) {
    ctx.last_error = code;
}

// =========================================================================
// Initialization
// =========================================================================

pub fn init_common_controls_ex(ctx: &mut CompatContext, icc: &InitCommonControlsExData) -> bool {
    if icc.size == 0 {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return false;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

// =========================================================================
// ListView functions
// =========================================================================

pub fn listview_insert_item(_hwnd: u64, item: &LvItem) -> i32 {
    item.item
}

pub fn listview_delete_item(_hwnd: u64, _index: i32) -> bool {
    true
}

pub fn listview_delete_all_items(_hwnd: u64) -> bool {
    true
}

pub fn listview_set_item(_hwnd: u64, _item: &LvItem) -> bool {
    true
}

pub fn listview_get_item(_hwnd: u64, item: &mut LvItem) -> bool {
    if item.mask & LVIF_TEXT != 0 {
        item.text = String::from("Item");
    }
    true
}

pub fn listview_get_item_count(_hwnd: u64) -> i32 {
    0
}

pub fn listview_insert_column(_hwnd: u64, index: i32, _column: &LvColumn) -> i32 {
    index
}

pub fn listview_set_column_width(_hwnd: u64, _col: i32, _cx: i32) -> bool {
    true
}

pub fn listview_get_selected_count(_hwnd: u64) -> u32 {
    0
}

pub fn listview_get_next_item(_hwnd: u64, _start: i32, _flags: u32) -> i32 {
    -1
}

pub fn listview_sort_items(_hwnd: u64, _compare: u64, _lparam: u64) -> bool {
    true
}

pub fn listview_hit_test(_hwnd: u64, info: &mut LvHitTestInfo) -> i32 {
    info.item = -1;
    info.sub_item = 0;
    info.flags = LVHT_NOWHERE;
    -1
}

pub fn listview_set_image_list(_hwnd: u64, _list: u64, _image_type: i32) -> u64 {
    0
}

pub fn listview_get_item_text(_hwnd: u64, _item: i32, _sub_item: i32) -> String {
    String::new()
}

pub fn listview_ensure_visible(_hwnd: u64, _index: i32, _partial_ok: bool) -> bool {
    true
}

// =========================================================================
// TreeView functions
// =========================================================================

fn alloc_tv_handle() -> u64 {
    static COUNTER: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0x00AA0001);
    COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
}

pub fn treeview_insert_item(_hwnd: u64, _insert: &TvInsertStruct) -> u64 {
    alloc_tv_handle()
}

pub fn treeview_delete_item(_hwnd: u64, _item: u64) -> bool {
    true
}

pub fn treeview_delete_all_items(_hwnd: u64) -> bool {
    true
}

pub fn treeview_get_item(_hwnd: u64, item: &mut TvItem) -> bool {
    if item.mask & TVIF_TEXT != 0 {
        item.text = String::from("TreeItem");
    }
    true
}

pub fn treeview_set_item(_hwnd: u64, _item: &TvItem) -> bool {
    true
}

pub fn treeview_select_item(_hwnd: u64, _item: u64) -> bool {
    true
}

pub fn treeview_expand(_hwnd: u64, _item: u64, _code: u32) -> bool {
    true
}

pub fn treeview_get_selection(_hwnd: u64) -> u64 {
    0
}

pub fn treeview_get_child(_hwnd: u64, _item: u64) -> u64 {
    0
}

pub fn treeview_get_parent(_hwnd: u64, _item: u64) -> u64 {
    0
}

pub fn treeview_get_next_sibling(_hwnd: u64, _item: u64) -> u64 {
    0
}

pub fn treeview_get_count(_hwnd: u64) -> u32 {
    0
}

// =========================================================================
// ToolBar functions
// =========================================================================

pub fn toolbar_add_buttons(_hwnd: u64, _buttons: &[TbButton]) -> bool {
    true
}

pub fn toolbar_insert_button(_hwnd: u64, _index: i32, _button: &TbButton) -> bool {
    true
}

pub fn toolbar_delete_button(_hwnd: u64, _index: i32) -> bool {
    true
}

pub fn toolbar_get_button(_hwnd: u64, _index: i32, button: &mut TbButton) -> bool {
    button.state = TBSTATE_ENABLED;
    true
}

pub fn toolbar_button_count(_hwnd: u64) -> i32 {
    0
}

pub fn toolbar_set_button_info(_hwnd: u64, _id: i32, _button: &TbButton) -> bool {
    true
}

pub fn toolbar_enable_button(_hwnd: u64, _id: i32, _enable: bool) -> bool {
    true
}

pub fn toolbar_check_button(_hwnd: u64, _id: i32, _check: bool) -> bool {
    true
}

pub fn toolbar_is_button_checked(_hwnd: u64, _id: i32) -> bool {
    false
}

pub fn toolbar_is_button_enabled(_hwnd: u64, _id: i32) -> bool {
    true
}

pub fn toolbar_set_image_list(_hwnd: u64, _list: u64) -> u64 {
    0
}

pub fn toolbar_autosize(_hwnd: u64) {}

// =========================================================================
// StatusBar functions
// =========================================================================

pub fn statusbar_set_parts(_hwnd: u64, _parts: &[i32]) -> bool {
    true
}

pub fn statusbar_set_text(_hwnd: u64, _part: i32, _text: &str) -> bool {
    true
}

pub fn statusbar_get_text(_hwnd: u64, _part: i32) -> String {
    String::new()
}

pub fn statusbar_get_text_length(_hwnd: u64, _part: i32) -> u32 {
    0
}

pub fn statusbar_get_parts(_hwnd: u64, parts: &mut [i32]) -> i32 {
    for p in parts.iter_mut() {
        *p = 0;
    }
    0
}

pub fn statusbar_set_min_height(_hwnd: u64, _height: i32) {}

pub fn statusbar_simple(_hwnd: u64, _simple: bool) -> bool {
    true
}

// =========================================================================
// ProgressBar functions
// =========================================================================

pub fn progressbar_set_range(_hwnd: u64, _low: i32, _high: i32) -> u32 {
    0
}

pub fn progressbar_set_pos(_hwnd: u64, pos: i32) -> i32 {
    pos
}

pub fn progressbar_delta_pos(_hwnd: u64, delta: i32) -> i32 {
    delta
}

pub fn progressbar_set_step(_hwnd: u64, _step: i32) -> i32 {
    0
}

pub fn progressbar_step_it(_hwnd: u64) -> i32 {
    0
}

pub fn progressbar_set_marquee(_hwnd: u64, _enable: bool, _anim_ms: u32) -> bool {
    true
}

pub fn progressbar_set_state(_hwnd: u64, _state: u32) -> u32 {
    PBST_NORMAL
}

pub fn progressbar_get_pos(_hwnd: u64) -> i32 {
    0
}

// =========================================================================
// Tab Control functions
// =========================================================================

pub fn tab_insert_item(_hwnd: u64, index: i32, _item: &TcItem) -> i32 {
    index
}

pub fn tab_delete_item(_hwnd: u64, _index: i32) -> bool {
    true
}

pub fn tab_delete_all_items(_hwnd: u64) -> bool {
    true
}

pub fn tab_get_item_count(_hwnd: u64) -> i32 {
    0
}

pub fn tab_get_cur_sel(_hwnd: u64) -> i32 {
    -1
}

pub fn tab_set_cur_sel(_hwnd: u64, _index: i32) -> i32 {
    -1
}

pub fn tab_get_item(_hwnd: u64, _index: i32, item: &mut TcItem) -> bool {
    item.text = String::from("Tab");
    true
}

pub fn tab_set_item(_hwnd: u64, _index: i32, _item: &TcItem) -> bool {
    true
}

pub fn tab_adjust_rect(_hwnd: u64, larger: bool, rect: &mut (i32, i32, i32, i32)) {
    if larger {
        rect.0 -= 4;
        rect.1 -= 24;
        rect.2 += 4;
        rect.3 += 4;
    } else {
        rect.0 += 4;
        rect.1 += 24;
        rect.2 -= 4;
        rect.3 -= 4;
    }
}

// =========================================================================
// Tooltip functions
// =========================================================================

pub fn tooltip_add_tool(_hwnd: u64, _info: &ToolInfo) -> bool {
    true
}

pub fn tooltip_del_tool(_hwnd: u64, _info: &ToolInfo) -> bool {
    true
}

pub fn tooltip_update_tip_text(_hwnd: u64, _info: &ToolInfo) -> bool {
    true
}

pub fn tooltip_get_text(_hwnd: u64, _info: &mut ToolInfo) -> bool {
    true
}

pub fn tooltip_set_max_tip_width(_hwnd: u64, _width: i32) -> i32 {
    -1
}

// =========================================================================
// ImageList functions
// =========================================================================

pub fn image_list_create(_cx: i32, _cy: i32, _flags: u32, _initial: i32, _grow: i32) -> u64 {
    static COUNTER: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0x00BB0001);
    COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
}

pub fn image_list_destroy(_handle: u64) -> bool {
    true
}

pub fn image_list_add(_handle: u64, _bitmap: u64, _mask: u64) -> i32 {
    0
}

pub fn image_list_add_icon(_handle: u64, _icon: u64) -> i32 {
    0
}

pub fn image_list_remove(_handle: u64, _index: i32) -> bool {
    true
}

pub fn image_list_remove_all(handle: u64) -> bool {
    image_list_remove(handle, -1)
}

pub fn image_list_get_image_count(_handle: u64) -> i32 {
    0
}

pub fn image_list_set_image_count(_handle: u64, _count: u32) -> bool {
    true
}

pub fn image_list_get_icon_size(_handle: u64, cx: &mut i32, cy: &mut i32) -> bool {
    *cx = 16;
    *cy = 16;
    true
}

pub fn image_list_set_icon_size(_handle: u64, _cx: i32, _cy: i32) -> bool {
    true
}

pub fn image_list_draw(
    _handle: u64,
    _index: i32,
    _hdc: u64,
    _x: i32,
    _y: i32,
    _style: u32,
) -> bool {
    true
}

// =========================================================================
// PropertySheet functions
// =========================================================================

pub fn property_sheet(ctx: &mut CompatContext, _header: &PropertySheetHeader) -> isize {
    set_last_error(ctx, ERROR_SUCCESS);
    1 // IDOK
}

pub fn create_property_sheet_page(_page: &PropertySheetPage) -> u64 {
    static COUNTER: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0x00CC0001);
    COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
}

pub fn destroy_property_sheet_page(_page: u64) -> bool {
    true
}

// =========================================================================
// Header Control functions
// =========================================================================

pub fn header_insert_item(_hwnd: u64, index: i32, _item: &HdItem) -> i32 {
    index
}

pub fn header_delete_item(_hwnd: u64, _index: i32) -> bool {
    true
}

pub fn header_get_item_count(_hwnd: u64) -> i32 {
    0
}

pub fn header_get_item(_hwnd: u64, _index: i32, item: &mut HdItem) -> bool {
    item.text = String::from("Header");
    true
}

pub fn header_set_item(_hwnd: u64, _index: i32, _item: &HdItem) -> bool {
    true
}

// =========================================================================
// Rebar functions
// =========================================================================

pub fn rebar_insert_band(_hwnd: u64, _index: i32, _band: &RebarBandInfo) -> bool {
    true
}

pub fn rebar_delete_band(_hwnd: u64, _index: u32) -> bool {
    true
}

pub fn rebar_get_band_count(_hwnd: u64) -> u32 {
    0
}

pub fn rebar_get_band_info(_hwnd: u64, _index: u32, info: &mut RebarBandInfo) -> bool {
    info.text = String::from("Band");
    true
}

pub fn rebar_set_band_info(_hwnd: u64, _index: u32, _info: &RebarBandInfo) -> bool {
    true
}

pub fn rebar_show_band(_hwnd: u64, _index: u32, _show: bool) -> bool {
    true
}

// =========================================================================
// UpDown Control functions
// =========================================================================

pub fn updown_set_range(_hwnd: u64, _low: i32, _high: i32) {}

pub fn updown_get_range(_hwnd: u64) -> (i32, i32) {
    (0, 100)
}

pub fn updown_set_pos(_hwnd: u64, _pos: i32) -> i32 {
    0
}

pub fn updown_get_pos(_hwnd: u64) -> i32 {
    0
}

pub fn updown_set_range32(_hwnd: u64, _low: i32, _high: i32) {}

pub fn updown_get_range32(_hwnd: u64, low: &mut i32, high: &mut i32) {
    *low = 0;
    *high = 100;
}

pub fn updown_set_pos32(_hwnd: u64, _pos: i32) -> i32 {
    0
}

pub fn updown_get_pos32(_hwnd: u64, _ok: &mut bool) -> i32 {
    0
}
