#![allow(dead_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use spin::Mutex;

// ---------------------------------------------------------------------------
// GPIO direction & flags
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpioDirection {
    Input,
    Output,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpioEdge {
    None,
    Rising,
    Falling,
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpioLevel {
    Low,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpioFlags(pub u32);

impl GpioFlags {
    pub const NONE: Self = Self(0);
    pub const ACTIVE_LOW: Self = Self(1 << 0);
    pub const OPEN_DRAIN: Self = Self(1 << 1);
    pub const OPEN_SOURCE: Self = Self(1 << 2);
    pub const PULL_UP: Self = Self(1 << 3);
    pub const PULL_DOWN: Self = Self(1 << 4);
    pub const BIAS_DISABLE: Self = Self(1 << 5);
    pub const DEBOUNCE: Self = Self(1 << 6);

    pub fn contains(&self, flag: GpioFlags) -> bool {
        self.0 & flag.0 == flag.0
    }

    pub fn set(&mut self, flag: GpioFlags) {
        self.0 |= flag.0;
    }

    pub fn clear(&mut self, flag: GpioFlags) {
        self.0 &= !flag.0;
    }
}

// ---------------------------------------------------------------------------
// GPIO line configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct GpioLineConfig {
    pub direction: GpioDirection,
    pub output_value: bool,
    pub flags: GpioFlags,
    pub debounce_us: u32,
}

impl GpioLineConfig {
    pub fn input() -> Self {
        Self {
            direction: GpioDirection::Input,
            output_value: false,
            flags: GpioFlags::NONE,
            debounce_us: 0,
        }
    }

    pub fn output(value: bool) -> Self {
        Self {
            direction: GpioDirection::Output,
            output_value: value,
            flags: GpioFlags::NONE,
            debounce_us: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// GPIO chip
// ---------------------------------------------------------------------------

pub type GpioIrqHandler = Box<dyn Fn(u32) + Send + Sync>;

pub struct GpioChip {
    pub id: u32,
    pub label: String,
    pub base: u32,
    pub ngpio: u32,
    pub lines: Vec<GpioLine>,
    pub owner: String,
    pub mmio_base: u64,
    pub irq_base: Option<u32>,
}

pub struct GpioLine {
    pub offset: u32,
    pub name: String,
    pub direction: GpioDirection,
    pub value: bool,
    pub flags: GpioFlags,
    pub active_low: bool,
    pub debounce_us: u32,
    pub consumer: Option<String>,
    pub exported: bool,
    pub irq_enabled: bool,
    pub irq_edge: GpioEdge,
    pub irq_level: Option<GpioLevel>,
    pub irq_handler: Option<GpioIrqHandler>,
    pub event_buffer: Vec<GpioLineEvent>,
    pub event_buffer_size: usize,
}

impl GpioLine {
    fn new(offset: u32) -> Self {
        Self {
            offset,
            name: String::new(),
            direction: GpioDirection::Input,
            value: false,
            flags: GpioFlags::NONE,
            active_low: false,
            debounce_us: 0,
            consumer: None,
            exported: false,
            irq_enabled: false,
            irq_edge: GpioEdge::None,
            irq_level: None,
            irq_handler: None,
            event_buffer: Vec::new(),
            event_buffer_size: 64,
        }
    }

    fn effective_value(&self) -> bool {
        if self.active_low {
            !self.value
        } else {
            self.value
        }
    }
}

impl GpioChip {
    pub fn new(label: &str, base: u32, ngpio: u32) -> Self {
        let mut lines = Vec::with_capacity(ngpio as usize);
        for i in 0..ngpio {
            lines.push(GpioLine::new(i));
        }
        Self {
            id: 0,
            label: String::from(label),
            base,
            ngpio,
            lines,
            owner: String::from("kernel"),
            mmio_base: 0,
            irq_base: None,
        }
    }

    pub fn direction_input(&mut self, offset: u32) -> Result<(), &'static str> {
        let line = self.get_line_mut(offset)?;
        line.direction = GpioDirection::Input;
        Ok(())
    }

    pub fn direction_output(&mut self, offset: u32, value: bool) -> Result<(), &'static str> {
        let line = self.get_line_mut(offset)?;
        line.direction = GpioDirection::Output;
        line.value = value;
        Ok(())
    }

    pub fn get_value(&self, offset: u32) -> Result<bool, &'static str> {
        let line = self.get_line(offset)?;
        Ok(line.effective_value())
    }

    pub fn set_value(&mut self, offset: u32, value: bool) -> Result<(), &'static str> {
        let line = self.get_line_mut(offset)?;
        if line.direction != GpioDirection::Output {
            return Err("GPIO line is not configured as output");
        }
        line.value = if line.active_low { !value } else { value };
        Ok(())
    }

    pub fn set_config(&mut self, offset: u32, config: &GpioLineConfig) -> Result<(), &'static str> {
        let line = self.get_line_mut(offset)?;
        line.direction = config.direction;
        line.flags = config.flags;
        line.debounce_us = config.debounce_us;
        line.active_low = config.flags.contains(GpioFlags::ACTIVE_LOW);
        if config.direction == GpioDirection::Output {
            line.value = config.output_value;
        }
        Ok(())
    }

    pub fn to_irq(&self, offset: u32) -> Option<u32> {
        self.irq_base.map(|base| base + offset)
    }

    fn get_line(&self, offset: u32) -> Result<&GpioLine, &'static str> {
        self.lines
            .get(offset as usize)
            .ok_or("GPIO offset out of range")
    }

    fn get_line_mut(&mut self, offset: u32) -> Result<&mut GpioLine, &'static str> {
        self.lines
            .get_mut(offset as usize)
            .ok_or("GPIO offset out of range")
    }
}

// ---------------------------------------------------------------------------
// GPIO descriptor API (gpiod)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct GpioDesc {
    pub chip_id: u32,
    pub offset: u32,
}

pub fn gpiod_get(chip_label: &str, offset: u32) -> Option<GpioDesc> {
    let subsys = GPIO_SUBSYSTEM.lock();
    let subsys = subsys.as_ref()?;
    let chip = subsys.chips.iter().find(|c| c.label == chip_label)?;
    if offset < chip.ngpio {
        Some(GpioDesc {
            chip_id: chip.id,
            offset,
        })
    } else {
        None
    }
}

pub fn gpiod_put(desc: &GpioDesc) {
    let mut subsys = GPIO_SUBSYSTEM.lock();
    if let Some(subsys) = subsys.as_mut() {
        if let Some(chip) = subsys.chips.iter_mut().find(|c| c.id == desc.chip_id) {
            if let Ok(line) = chip.get_line_mut(desc.offset) {
                line.consumer = None;
            }
        }
    }
}

pub fn gpiod_direction_input(desc: &GpioDesc) -> Result<(), &'static str> {
    let mut subsys = GPIO_SUBSYSTEM.lock();
    let subsys = subsys.as_mut().ok_or("GPIO subsystem not initialized")?;
    let chip = subsys
        .chips
        .iter_mut()
        .find(|c| c.id == desc.chip_id)
        .ok_or("chip not found")?;
    chip.direction_input(desc.offset)
}

pub fn gpiod_direction_output(desc: &GpioDesc, value: bool) -> Result<(), &'static str> {
    let mut subsys = GPIO_SUBSYSTEM.lock();
    let subsys = subsys.as_mut().ok_or("GPIO subsystem not initialized")?;
    let chip = subsys
        .chips
        .iter_mut()
        .find(|c| c.id == desc.chip_id)
        .ok_or("chip not found")?;
    chip.direction_output(desc.offset, value)
}

pub fn gpiod_get_value(desc: &GpioDesc) -> Result<bool, &'static str> {
    let subsys = GPIO_SUBSYSTEM.lock();
    let subsys = subsys.as_ref().ok_or("GPIO subsystem not initialized")?;
    let chip = subsys
        .chips
        .iter()
        .find(|c| c.id == desc.chip_id)
        .ok_or("chip not found")?;
    chip.get_value(desc.offset)
}

pub fn gpiod_set_value(desc: &GpioDesc, value: bool) -> Result<(), &'static str> {
    let mut subsys = GPIO_SUBSYSTEM.lock();
    let subsys = subsys.as_mut().ok_or("GPIO subsystem not initialized")?;
    let chip = subsys
        .chips
        .iter_mut()
        .find(|c| c.id == desc.chip_id)
        .ok_or("chip not found")?;
    chip.set_value(desc.offset, value)
}

pub fn gpiod_set_debounce(desc: &GpioDesc, debounce_us: u32) -> Result<(), &'static str> {
    let mut subsys = GPIO_SUBSYSTEM.lock();
    let subsys = subsys.as_mut().ok_or("GPIO subsystem not initialized")?;
    let chip = subsys
        .chips
        .iter_mut()
        .find(|c| c.id == desc.chip_id)
        .ok_or("chip not found")?;
    let line = chip.get_line_mut(desc.offset)?;
    line.debounce_us = debounce_us;
    line.flags.set(GpioFlags::DEBOUNCE);
    Ok(())
}

pub fn gpiod_to_irq(desc: &GpioDesc) -> Option<u32> {
    let subsys = GPIO_SUBSYSTEM.lock();
    let subsys = subsys.as_ref()?;
    let chip = subsys.chips.iter().find(|c| c.id == desc.chip_id)?;
    chip.to_irq(desc.offset)
}

pub fn gpiod_is_active_low(desc: &GpioDesc) -> bool {
    let subsys = GPIO_SUBSYSTEM.lock();
    subsys
        .as_ref()
        .and_then(|s| s.chips.iter().find(|c| c.id == desc.chip_id))
        .and_then(|c| c.get_line(desc.offset).ok())
        .map(|l| l.active_low)
        .unwrap_or(false)
}

pub fn gpiod_toggle_active_low(desc: &GpioDesc) -> Result<(), &'static str> {
    let mut subsys = GPIO_SUBSYSTEM.lock();
    let subsys = subsys.as_mut().ok_or("GPIO subsystem not initialized")?;
    let chip = subsys
        .chips
        .iter_mut()
        .find(|c| c.id == desc.chip_id)
        .ok_or("chip not found")?;
    let line = chip.get_line_mut(desc.offset)?;
    line.active_low = !line.active_low;
    Ok(())
}

// ---------------------------------------------------------------------------
// GPIO consumer API
// ---------------------------------------------------------------------------

pub fn gpio_request(gpio_num: u32, label: &str) -> Result<(), &'static str> {
    let mut subsys = GPIO_SUBSYSTEM.lock();
    let subsys = subsys.as_mut().ok_or("GPIO subsystem not initialized")?;
    for chip in &mut subsys.chips {
        if gpio_num >= chip.base && gpio_num < chip.base + chip.ngpio {
            let offset = gpio_num - chip.base;
            let line = chip.get_line_mut(offset)?;
            if line.consumer.is_some() {
                return Err("GPIO already requested");
            }
            line.consumer = Some(String::from(label));
            return Ok(());
        }
    }
    Err("GPIO number not found")
}

pub fn gpio_free(gpio_num: u32) {
    let mut subsys = GPIO_SUBSYSTEM.lock();
    if let Some(subsys) = subsys.as_mut() {
        for chip in &mut subsys.chips {
            if gpio_num >= chip.base && gpio_num < chip.base + chip.ngpio {
                let offset = gpio_num - chip.base;
                if let Ok(line) = chip.get_line_mut(offset) {
                    line.consumer = None;
                    line.irq_enabled = false;
                    line.irq_handler = None;
                }
                return;
            }
        }
    }
}

pub fn devm_gpiod_get(chip_label: &str, con_id: &str, flags: GpioFlags) -> Option<GpioDesc> {
    let desc = gpiod_get(chip_label, 0)?;
    let mut subsys = GPIO_SUBSYSTEM.lock();
    if let Some(subsys) = subsys.as_mut() {
        if let Some(chip) = subsys.chips.iter_mut().find(|c| c.id == desc.chip_id) {
            if let Ok(line) = chip.get_line_mut(desc.offset) {
                line.consumer = Some(String::from(con_id));
                line.flags = flags;
                line.active_low = flags.contains(GpioFlags::ACTIVE_LOW);
            }
        }
    }
    Some(desc)
}

pub fn devm_gpiod_get_optional(
    chip_label: &str,
    con_id: &str,
    flags: GpioFlags,
) -> Option<GpioDesc> {
    devm_gpiod_get(chip_label, con_id, flags)
}

pub fn gpiod_get_array(chip_label: &str, offsets: &[u32]) -> Vec<GpioDesc> {
    offsets
        .iter()
        .filter_map(|&off| gpiod_get(chip_label, off))
        .collect()
}

// ---------------------------------------------------------------------------
// GPIO IRQ support
// ---------------------------------------------------------------------------

pub fn gpio_irq_set_edge(desc: &GpioDesc, edge: GpioEdge) -> Result<(), &'static str> {
    let mut subsys = GPIO_SUBSYSTEM.lock();
    let subsys = subsys.as_mut().ok_or("GPIO subsystem not initialized")?;
    let chip = subsys
        .chips
        .iter_mut()
        .find(|c| c.id == desc.chip_id)
        .ok_or("chip not found")?;
    let line = chip.get_line_mut(desc.offset)?;
    line.irq_edge = edge;
    line.irq_enabled = edge != GpioEdge::None;
    Ok(())
}

pub fn gpio_irq_set_level(desc: &GpioDesc, level: GpioLevel) -> Result<(), &'static str> {
    let mut subsys = GPIO_SUBSYSTEM.lock();
    let subsys = subsys.as_mut().ok_or("GPIO subsystem not initialized")?;
    let chip = subsys
        .chips
        .iter_mut()
        .find(|c| c.id == desc.chip_id)
        .ok_or("chip not found")?;
    let line = chip.get_line_mut(desc.offset)?;
    line.irq_level = Some(level);
    line.irq_enabled = true;
    Ok(())
}

pub fn gpio_irq_set_handler(desc: &GpioDesc, handler: GpioIrqHandler) -> Result<(), &'static str> {
    let mut subsys = GPIO_SUBSYSTEM.lock();
    let subsys = subsys.as_mut().ok_or("GPIO subsystem not initialized")?;
    let chip = subsys
        .chips
        .iter_mut()
        .find(|c| c.id == desc.chip_id)
        .ok_or("chip not found")?;
    let line = chip.get_line_mut(desc.offset)?;
    line.irq_handler = Some(handler);
    Ok(())
}

// ---------------------------------------------------------------------------
// GPIO line events
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpioEventType {
    RisingEdge,
    FallingEdge,
}

#[derive(Debug, Clone, Copy)]
pub struct GpioLineEvent {
    pub timestamp_ns: u64,
    pub event_type: GpioEventType,
    pub line_offset: u32,
    pub seqno: u32,
}

static EVENT_SEQNO: AtomicU32 = AtomicU32::new(1);

pub fn gpio_push_event(desc: &GpioDesc, event_type: GpioEventType, timestamp_ns: u64) {
    let mut subsys = GPIO_SUBSYSTEM.lock();
    if let Some(subsys) = subsys.as_mut() {
        if let Some(chip) = subsys.chips.iter_mut().find(|c| c.id == desc.chip_id) {
            if let Ok(line) = chip.get_line_mut(desc.offset) {
                let event = GpioLineEvent {
                    timestamp_ns,
                    event_type,
                    line_offset: desc.offset,
                    seqno: EVENT_SEQNO.fetch_add(1, Ordering::Relaxed),
                };
                if line.event_buffer.len() >= line.event_buffer_size {
                    line.event_buffer.remove(0);
                }
                line.event_buffer.push(event);
            }
        }
    }
}

pub fn gpio_poll_events(desc: &GpioDesc) -> Vec<GpioLineEvent> {
    let mut subsys = GPIO_SUBSYSTEM.lock();
    if let Some(subsys) = subsys.as_mut() {
        if let Some(chip) = subsys.chips.iter_mut().find(|c| c.id == desc.chip_id) {
            if let Ok(line) = chip.get_line_mut(desc.offset) {
                let events = line.event_buffer.clone();
                line.event_buffer.clear();
                return events;
            }
        }
    }
    Vec::new()
}

// ---------------------------------------------------------------------------
// GPIO sysfs/chardev: export/unexport
// ---------------------------------------------------------------------------

pub fn gpio_export(gpio_num: u32) -> Result<(), &'static str> {
    let mut subsys = GPIO_SUBSYSTEM.lock();
    let subsys = subsys.as_mut().ok_or("GPIO subsystem not initialized")?;
    for chip in &mut subsys.chips {
        if gpio_num >= chip.base && gpio_num < chip.base + chip.ngpio {
            let offset = gpio_num - chip.base;
            let line = chip.get_line_mut(offset)?;
            line.exported = true;
            return Ok(());
        }
    }
    Err("GPIO number not found")
}

pub fn gpio_unexport(gpio_num: u32) {
    let mut subsys = GPIO_SUBSYSTEM.lock();
    if let Some(subsys) = subsys.as_mut() {
        for chip in &mut subsys.chips {
            if gpio_num >= chip.base && gpio_num < chip.base + chip.ngpio {
                let offset = gpio_num - chip.base;
                if let Ok(line) = chip.get_line_mut(offset) {
                    line.exported = false;
                }
                return;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// GPIO aggregator — virtual GPIO chips
// ---------------------------------------------------------------------------

pub struct GpioAggregator {
    pub id: u32,
    pub name: String,
    pub mappings: Vec<AggregatorMapping>,
}

#[derive(Debug, Clone)]
pub struct AggregatorMapping {
    pub virtual_offset: u32,
    pub chip_id: u32,
    pub physical_offset: u32,
}

impl GpioAggregator {
    pub fn new(name: &str) -> Self {
        Self {
            id: 0,
            name: String::from(name),
            mappings: Vec::new(),
        }
    }

    pub fn add_mapping(&mut self, virtual_offset: u32, chip_id: u32, physical_offset: u32) {
        self.mappings.push(AggregatorMapping {
            virtual_offset,
            chip_id,
            physical_offset,
        });
    }

    pub fn resolve(&self, virtual_offset: u32) -> Option<(u32, u32)> {
        self.mappings
            .iter()
            .find(|m| m.virtual_offset == virtual_offset)
            .map(|m| (m.chip_id, m.physical_offset))
    }
}

// ---------------------------------------------------------------------------
// GPIO multiplexer
// ---------------------------------------------------------------------------

pub struct GpioMux {
    pub id: u32,
    pub name: String,
    pub select_pins: Vec<GpioDesc>,
    pub current_state: u32,
    pub num_states: u32,
}

impl GpioMux {
    pub fn new(name: &str, select_pins: Vec<GpioDesc>, num_states: u32) -> Self {
        Self {
            id: 0,
            name: String::from(name),
            select_pins,
            current_state: 0,
            num_states,
        }
    }

    pub fn select(&mut self, state: u32) -> Result<(), &'static str> {
        if state >= self.num_states {
            return Err("mux state out of range");
        }
        for (i, pin) in self.select_pins.iter().enumerate() {
            let bit = (state >> i) & 1;
            let _ = gpiod_set_value(pin, bit != 0);
        }
        self.current_state = state;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// GPIO expander (I2C/SPI)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpanderType {
    Pca9535,
    Mcp23017,
    Pcf8574,
}

pub struct GpioExpander {
    pub expander_type: ExpanderType,
    pub bus_addr: u8,
    pub chip_id: u32,
    pub ngpio: u32,
    pub direction_cache: u32,
    pub output_cache: u32,
    pub input_cache: u32,
    pub irq_mask: u32,
}

impl GpioExpander {
    pub fn new(exp_type: ExpanderType, bus_addr: u8) -> Self {
        let ngpio = match exp_type {
            ExpanderType::Pca9535 => 16,
            ExpanderType::Mcp23017 => 16,
            ExpanderType::Pcf8574 => 8,
        };
        Self {
            expander_type: exp_type,
            bus_addr,
            chip_id: 0,
            ngpio,
            direction_cache: 0xFFFF_FFFF,
            output_cache: 0,
            input_cache: 0,
            irq_mask: 0,
        }
    }

    pub fn set_direction(&mut self, pin: u32, dir: GpioDirection) {
        if dir == GpioDirection::Output {
            self.direction_cache &= !(1 << pin);
        } else {
            self.direction_cache |= 1 << pin;
        }
    }

    pub fn set_output(&mut self, pin: u32, value: bool) {
        if value {
            self.output_cache |= 1 << pin;
        } else {
            self.output_cache &= !(1 << pin);
        }
    }

    pub fn get_input(&self, pin: u32) -> bool {
        (self.input_cache >> pin) & 1 != 0
    }

    pub fn enable_irq(&mut self, pin: u32) {
        self.irq_mask |= 1 << pin;
    }

    pub fn disable_irq(&mut self, pin: u32) {
        self.irq_mask &= !(1 << pin);
    }
}

// ---------------------------------------------------------------------------
// LED class
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedTrigger {
    None,
    Heartbeat,
    Timer,
    DiskActivity,
    Cpu,
    DefaultOn,
    Panic,
    Netdev,
}

pub struct Led {
    pub id: u32,
    pub name: String,
    pub brightness: u8,
    pub max_brightness: u8,
    pub trigger: LedTrigger,
    pub blink_delay_on_ms: u32,
    pub blink_delay_off_ms: u32,
    pub gpio_desc: Option<GpioDesc>,
    pub group: Option<u32>,
}

impl Led {
    pub fn new(name: &str, max_brightness: u8) -> Self {
        Self {
            id: 0,
            name: String::from(name),
            brightness: 0,
            max_brightness,
            trigger: LedTrigger::None,
            blink_delay_on_ms: 500,
            blink_delay_off_ms: 500,
            gpio_desc: None,
            group: None,
        }
    }

    pub fn set_brightness(&mut self, val: u8) {
        self.brightness = core::cmp::min(val, self.max_brightness);
        if let Some(ref desc) = self.gpio_desc {
            let _ = gpiod_set_value(desc, self.brightness > 0);
        }
    }

    pub fn get_brightness(&self) -> u8 {
        self.brightness
    }

    pub fn set_blink(&mut self, delay_on_ms: u32, delay_off_ms: u32) {
        self.blink_delay_on_ms = delay_on_ms;
        self.blink_delay_off_ms = delay_off_ms;
        self.trigger = LedTrigger::Timer;
    }

    pub fn set_trigger(&mut self, trigger: LedTrigger) {
        self.trigger = trigger;
        match trigger {
            LedTrigger::DefaultOn => self.set_brightness(self.max_brightness),
            LedTrigger::Panic => {}
            _ => {}
        }
    }
}

pub struct LedGroup {
    pub id: u32,
    pub name: String,
    pub led_ids: Vec<u32>,
}

// ---------------------------------------------------------------------------
// Key/button class
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyState {
    Released,
    Pressed,
}

pub struct InputKey {
    pub id: u32,
    pub name: String,
    pub code: u32,
    pub state: KeyState,
    pub debounce_ms: u32,
    pub repeat_delay_ms: u32,
    pub repeat_rate_ms: u32,
    pub wakeup: bool,
    pub gpio_desc: Option<GpioDesc>,
    pub active_low: bool,
    pub last_change_ns: u64,
}

impl InputKey {
    pub fn new(name: &str, code: u32) -> Self {
        Self {
            id: 0,
            name: String::from(name),
            code,
            state: KeyState::Released,
            debounce_ms: 20,
            repeat_delay_ms: 500,
            repeat_rate_ms: 33,
            wakeup: false,
            gpio_desc: None,
            active_low: false,
            last_change_ns: 0,
        }
    }

    pub fn poll(&mut self) -> Option<KeyState> {
        if let Some(ref desc) = self.gpio_desc {
            if let Ok(val) = gpiod_get_value(desc) {
                let pressed = if self.active_low { !val } else { val };
                let new_state = if pressed {
                    KeyState::Pressed
                } else {
                    KeyState::Released
                };
                if new_state != self.state {
                    self.state = new_state;
                    return Some(new_state);
                }
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// PWM framework
// ---------------------------------------------------------------------------

pub struct PwmChip {
    pub id: u32,
    pub label: String,
    pub npwm: u32,
    pub devices: Vec<PwmDevice>,
    pub base_addr: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PwmPolarity {
    Normal,
    Inversed,
}

pub struct PwmDevice {
    pub hwpwm: u32,
    pub label: String,
    pub period_ns: u64,
    pub duty_ns: u64,
    pub polarity: PwmPolarity,
    pub enabled: bool,
    pub requested: bool,
}

impl PwmChip {
    pub fn new(label: &str, npwm: u32) -> Self {
        let mut devices = Vec::with_capacity(npwm as usize);
        for i in 0..npwm {
            devices.push(PwmDevice {
                hwpwm: i,
                label: String::new(),
                period_ns: 0,
                duty_ns: 0,
                polarity: PwmPolarity::Normal,
                enabled: false,
                requested: false,
            });
        }
        Self {
            id: 0,
            label: String::from(label),
            npwm,
            devices,
            base_addr: 0,
        }
    }

    pub fn request(&mut self, hwpwm: u32, label: &str) -> Result<(), &'static str> {
        let dev = self
            .devices
            .get_mut(hwpwm as usize)
            .ok_or("PWM index out of range")?;
        if dev.requested {
            return Err("PWM already requested");
        }
        dev.requested = true;
        dev.label = String::from(label);
        Ok(())
    }

    pub fn free(&mut self, hwpwm: u32) -> Result<(), &'static str> {
        let dev = self
            .devices
            .get_mut(hwpwm as usize)
            .ok_or("PWM index out of range")?;
        dev.requested = false;
        dev.enabled = false;
        dev.label = String::new();
        Ok(())
    }

    pub fn config(&mut self, hwpwm: u32, period_ns: u64, duty_ns: u64) -> Result<(), &'static str> {
        let dev = self
            .devices
            .get_mut(hwpwm as usize)
            .ok_or("PWM index out of range")?;
        if duty_ns > period_ns {
            return Err("duty cycle exceeds period");
        }
        dev.period_ns = period_ns;
        dev.duty_ns = duty_ns;
        Ok(())
    }

    pub fn set_polarity(&mut self, hwpwm: u32, pol: PwmPolarity) -> Result<(), &'static str> {
        let dev = self
            .devices
            .get_mut(hwpwm as usize)
            .ok_or("PWM index out of range")?;
        if dev.enabled {
            return Err("cannot change polarity while enabled");
        }
        dev.polarity = pol;
        Ok(())
    }

    pub fn enable(&mut self, hwpwm: u32) -> Result<(), &'static str> {
        let dev = self
            .devices
            .get_mut(hwpwm as usize)
            .ok_or("PWM index out of range")?;
        dev.enabled = true;
        Ok(())
    }

    pub fn disable(&mut self, hwpwm: u32) -> Result<(), &'static str> {
        let dev = self
            .devices
            .get_mut(hwpwm as usize)
            .ok_or("PWM index out of range")?;
        dev.enabled = false;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Pinctrl framework
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinBias {
    Disabled,
    PullUp,
    PullDown,
    HighImpedance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinDriveStrength {
    Strength2mA,
    Strength4mA,
    Strength8mA,
    Strength12mA,
    Strength16mA,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlewRate {
    Slow,
    Fast,
}

#[derive(Debug, Clone)]
pub struct PinConfig {
    pub bias: PinBias,
    pub drive_strength: PinDriveStrength,
    pub slew_rate: SlewRate,
    pub schmitt_trigger: bool,
    pub input_enable: bool,
    pub output_enable: bool,
}

impl PinConfig {
    pub fn default_config() -> Self {
        Self {
            bias: PinBias::Disabled,
            drive_strength: PinDriveStrength::Strength4mA,
            slew_rate: SlewRate::Slow,
            schmitt_trigger: false,
            input_enable: true,
            output_enable: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PinFunction {
    pub name: String,
    pub groups: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PinGroup {
    pub name: String,
    pub pins: Vec<u32>,
}

pub struct PinController {
    pub id: u32,
    pub name: String,
    pub npins: u32,
    pub configs: Vec<PinConfig>,
    pub functions: Vec<PinFunction>,
    pub groups: Vec<PinGroup>,
    pub pin_names: Vec<String>,
    pub device_maps: Vec<DevicePinMap>,
}

#[derive(Debug, Clone)]
pub struct DevicePinMap {
    pub device_name: String,
    pub state_name: String,
    pub function: String,
    pub group: String,
}

impl PinController {
    pub fn new(name: &str, npins: u32) -> Self {
        let mut configs = Vec::with_capacity(npins as usize);
        let mut pin_names = Vec::with_capacity(npins as usize);
        for i in 0..npins {
            configs.push(PinConfig::default_config());
            pin_names.push(alloc::format!("pin_{}", i));
        }
        Self {
            id: 0,
            name: String::from(name),
            npins,
            configs,
            functions: Vec::new(),
            groups: Vec::new(),
            pin_names,
            device_maps: Vec::new(),
        }
    }

    pub fn add_function(&mut self, name: &str, groups: &[&str]) {
        self.functions.push(PinFunction {
            name: String::from(name),
            groups: groups.iter().map(|g| String::from(*g)).collect(),
        });
    }

    pub fn add_group(&mut self, name: &str, pins: &[u32]) {
        self.groups.push(PinGroup {
            name: String::from(name),
            pins: pins.to_vec(),
        });
    }

    pub fn set_mux(&mut self, function: &str, group: &str) -> Result<(), &'static str> {
        let func = self
            .functions
            .iter()
            .find(|f| f.name == function)
            .ok_or("function not found")?;
        if !func.groups.iter().any(|g| g == group) {
            return Err("group not valid for function");
        }
        Ok(())
    }

    pub fn set_pin_config(&mut self, pin: u32, config: PinConfig) -> Result<(), &'static str> {
        let cfg = self
            .configs
            .get_mut(pin as usize)
            .ok_or("pin out of range")?;
        *cfg = config;
        Ok(())
    }

    pub fn get_pin_config(&self, pin: u32) -> Result<&PinConfig, &'static str> {
        self.configs.get(pin as usize).ok_or("pin out of range")
    }

    pub fn add_device_map(&mut self, device: &str, state: &str, function: &str, group: &str) {
        self.device_maps.push(DevicePinMap {
            device_name: String::from(device),
            state_name: String::from(state),
            function: String::from(function),
            group: String::from(group),
        });
    }

    pub fn apply_device_state(&mut self, device: &str, state: &str) -> Result<(), &'static str> {
        let maps: Vec<DevicePinMap> = self
            .device_maps
            .iter()
            .filter(|m| m.device_name == device && m.state_name == state)
            .cloned()
            .collect();
        if maps.is_empty() {
            return Err("no pin mapping found for device/state");
        }
        for map in &maps {
            self.set_mux(&map.function, &map.group)?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// GPIO subsystem (global state)
// ---------------------------------------------------------------------------

struct GpioSubsystem {
    chips: Vec<GpioChip>,
    aggregators: Vec<GpioAggregator>,
    muxes: Vec<GpioMux>,
    expanders: Vec<GpioExpander>,
    leds: Vec<Led>,
    led_groups: Vec<LedGroup>,
    keys: Vec<InputKey>,
    pwm_chips: Vec<PwmChip>,
    pin_controllers: Vec<PinController>,
    next_chip_id: u32,
    next_agg_id: u32,
    next_mux_id: u32,
    next_led_id: u32,
    next_key_id: u32,
    next_pwm_id: u32,
    next_pinctrl_id: u32,
}

impl GpioSubsystem {
    fn new() -> Self {
        Self {
            chips: Vec::new(),
            aggregators: Vec::new(),
            muxes: Vec::new(),
            expanders: Vec::new(),
            leds: Vec::new(),
            led_groups: Vec::new(),
            keys: Vec::new(),
            pwm_chips: Vec::new(),
            pin_controllers: Vec::new(),
            next_chip_id: 1,
            next_agg_id: 1,
            next_mux_id: 1,
            next_led_id: 1,
            next_key_id: 1,
            next_pwm_id: 1,
            next_pinctrl_id: 1,
        }
    }

    fn register_chip(&mut self, mut chip: GpioChip) -> u32 {
        let id = self.next_chip_id;
        self.next_chip_id += 1;
        chip.id = id;
        self.chips.push(chip);
        id
    }

    fn unregister_chip(&mut self, id: u32) {
        self.chips.retain(|c| c.id != id);
    }

    fn register_aggregator(&mut self, mut agg: GpioAggregator) -> u32 {
        let id = self.next_agg_id;
        self.next_agg_id += 1;
        agg.id = id;
        self.aggregators.push(agg);
        id
    }

    fn register_mux(&mut self, mut mux: GpioMux) -> u32 {
        let id = self.next_mux_id;
        self.next_mux_id += 1;
        mux.id = id;
        self.muxes.push(mux);
        id
    }

    fn register_expander(&mut self, mut exp: GpioExpander, base: u32) -> u32 {
        let chip = GpioChip::new(
            &alloc::format!("{:?}@{:#x}", exp.expander_type, exp.bus_addr),
            base,
            exp.ngpio,
        );
        let chip_id = self.register_chip(chip);
        exp.chip_id = chip_id;
        self.expanders.push(exp);
        chip_id
    }

    fn register_led(&mut self, mut led: Led) -> u32 {
        let id = self.next_led_id;
        self.next_led_id += 1;
        led.id = id;
        self.leds.push(led);
        id
    }

    fn register_key(&mut self, mut key: InputKey) -> u32 {
        let id = self.next_key_id;
        self.next_key_id += 1;
        key.id = id;
        self.keys.push(key);
        id
    }

    fn register_pwm_chip(&mut self, mut chip: PwmChip) -> u32 {
        let id = self.next_pwm_id;
        self.next_pwm_id += 1;
        chip.id = id;
        self.pwm_chips.push(chip);
        id
    }

    fn register_pin_controller(&mut self, mut ctrl: PinController) -> u32 {
        let id = self.next_pinctrl_id;
        self.next_pinctrl_id += 1;
        ctrl.id = id;
        self.pin_controllers.push(ctrl);
        id
    }
}

pub static GPIO_SUBSYSTEM: Mutex<Option<GpioSubsystem>> = Mutex::new(None);

pub fn init() {
    let mut subsys = GPIO_SUBSYSTEM.lock();
    let mut s = GpioSubsystem::new();

    let chip0 = GpioChip::new("gpio0", 0, 32);
    s.register_chip(chip0);

    let chip1 = GpioChip::new("gpio1", 32, 32);
    s.register_chip(chip1);

    let mut pinctrl = PinController::new("soc-pinctrl", 64);
    pinctrl.add_function("uart0", &["uart0-pins"]);
    pinctrl.add_function("spi0", &["spi0-pins"]);
    pinctrl.add_function("i2c0", &["i2c0-pins"]);
    pinctrl.add_group("uart0-pins", &[14, 15]);
    pinctrl.add_group("spi0-pins", &[7, 8, 9, 10]);
    pinctrl.add_group("i2c0-pins", &[2, 3]);
    s.register_pin_controller(pinctrl);

    let pwm = PwmChip::new("pwm0", 4);
    s.register_pwm_chip(pwm);

    let mut status_led = Led::new("status", 1);
    status_led.set_trigger(LedTrigger::Heartbeat);
    s.register_led(status_led);

    let mut power_led = Led::new("power", 1);
    power_led.set_trigger(LedTrigger::DefaultOn);
    s.register_led(power_led);

    let power_btn = InputKey::new("power-button", 116);
    s.register_key(power_btn);

    *subsys = Some(s);
}
