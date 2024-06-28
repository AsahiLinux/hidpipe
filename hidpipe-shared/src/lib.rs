use std::ffi::c_char;
use input_linux::{
    InputProperty, EventKind, AbsoluteAxis, Key, RelativeAxis,
    MiscKind, LedKind, SoundKind, SwitchKind, InputId, bitmask::BitmaskTrait
};
use input_linux::sys::input_event;

#[repr(C)]
#[derive(Debug)]
pub struct ClientHello {
    pub version: u32
}

#[repr(C)]
#[derive(Debug)]
pub struct ServerHello {
    pub version: u32
}

#[repr(u32)]
#[derive(Debug)]
pub enum MessageType {
    AddDevice,
    RemoveDevice,
    InputEvent
}

#[repr(C)]
#[derive(Debug)]
pub struct AddDevice {
    pub id: u32,
    pub evbits: <EventKind as BitmaskTrait>::Array,
    pub keybits: <Key as BitmaskTrait>::Array,
    pub relbits: <RelativeAxis as BitmaskTrait>::Array,
    pub absbits: <AbsoluteAxis  as BitmaskTrait>::Array,
    pub mscbits: <MiscKind as BitmaskTrait>::Array,
    pub ledbits: <LedKind as BitmaskTrait>::Array,
    pub sndbits: <SoundKind as BitmaskTrait>::Array,
    pub swbits: <SwitchKind as BitmaskTrait>::Array,
    pub propbits: <InputProperty as BitmaskTrait>::Array,
    pub input_id: InputId,
    pub ff_effects: u32,
    pub name: [c_char; 80],
}

#[repr(C)]
#[derive(Debug)]
pub struct RemoveDevice {
    pub id: u32
}

#[repr(C)]
#[derive(Debug)]
pub struct InputEvent {
    pub time_sec: i64,
    pub time_usec: i64,
    pub value: i32,
    pub id: u32,
    pub ty: u16,
    pub code: u16,
}

impl InputEvent {
    pub fn new(id: u32, e: input_event) -> InputEvent {
        InputEvent {
            id,
            ty: e.type_,
            code: e.code,
            value: e.value,
            time_sec: e.time.tv_sec,
            time_usec: e.time.tv_usec,
        }
    }
}

