use std::ffi::c_char;
use std::{slice, mem};
use std::io::{Result, Write};
use std::os::unix::net::UnixStream;
use input_linux::{
    InputProperty, EventKind, AbsoluteAxis, Key, RelativeAxis,
    MiscKind, LedKind, SoundKind, SwitchKind, InputId, bitmask::BitmaskTrait
};
use input_linux::sys::{input_event, timeval};

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
    pub id: u64,
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
    pub id: u64
}

#[repr(C)]
#[derive(Debug)]
pub struct InputEvent {
    pub time_sec: i64,
    pub time_usec: i64,
    pub id: u64,
    pub value: i32,
    pub ty: u16,
    pub code: u16,
}

impl InputEvent {
    pub fn new(id: u64, e: input_event) -> InputEvent {
        InputEvent {
            id,
            ty: e.type_,
            code: e.code,
            value: e.value,
            time_sec: e.time.tv_sec,
            time_usec: e.time.tv_usec,
        }
    }
    pub fn to_input_event(&self) -> input_event {
        input_event {
            time: timeval {
                tv_sec: self.time_sec,
                tv_usec: self.time_usec
            },
            type_: self.ty,
            code: self.code,
            value: self.value,
        }
    }
}

pub fn empty_input_event() -> input_event {
    input_event {
        time: timeval { tv_sec: 0, tv_usec: 0 },
        type_: 0,
        code: 0,
        value: 0,
    }
}

pub fn struct_to_socket<T>(socket: &mut UnixStream, data: &mut T) -> Result<()> {
    let size = mem::size_of::<T>();
    // SAFETY:
    // 1. We are taking a ref, so it is valid for reads and properly aligned
    // 2. We are taking a mut ref, so nobody but us can write to it
    let v = unsafe {
        slice::from_raw_parts(data as *mut T as *const u8, size)
    };
    socket.write_all(v)
}
