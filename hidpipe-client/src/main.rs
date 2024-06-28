use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::{mem, slice};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use input_linux::{
    AbsoluteAxis, AbsoluteInfo, Bitmask, EventKind, InputProperty, Key,
    LedKind, MiscKind, RelativeAxis, SoundKind, SwitchKind, UInputHandle
};
use input_linux::bitmask::BitmaskTrait;
use input_linux_sys::{uinput_setup, input_event, input_id, uinput_abs_setup, input_absinfo, timeval};
use nix::sys::socket::{AddressFamily, connect, socket, SockFlag, SockType, VsockAddr};
use hidpipe_shared::{AddDevice, ClientHello, MessageType, RemoveDevice, ServerHello, InputEvent};

const ADD_DEVICE: u32 = MessageType::AddDevice as u32;
const REMOVE_DEVICE: u32 = MessageType::RemoveDevice as u32;
const INPUT_EVENT: u32 = MessageType::InputEvent as u32;

fn bitmask_from_slice<T, A>(s: &T::Array) -> Bitmask<T> where
    A: AsRef<[u8]>, T: BitmaskTrait<Array = A> {
    let mut bm = Bitmask::<T>::default();
    bm.copy_from_slice(s.as_ref());
    bm
}

fn main() {
    let sock_fd = socket(AddressFamily::Vsock, SockType::Stream, SockFlag::empty(), None).unwrap();
    connect(sock_fd.as_raw_fd(), &VsockAddr::new(2, 3334)).unwrap();
    let mut sock = UnixStream::from(sock_fd);
    let c_hello = ClientHello {
        version: 0
    };
    let c_hello_data = unsafe {
        slice::from_raw_parts(&c_hello as *const ClientHello as *const u8, mem::size_of::<ClientHello>())
    };
    sock.write_all(c_hello_data).unwrap();
    let mut s_hello_data = [0u8; mem::size_of::<ServerHello>()];
    sock.read_exact(&mut s_hello_data).unwrap();
    let mut inputs = HashMap::new();
    loop {
        let mut cmd_data = [0u8; mem::size_of::<MessageType>()];
        sock.read_exact(&mut cmd_data).unwrap();
        match u32::from_ne_bytes(cmd_data) {
            ADD_DEVICE => {
                let mut add_dev_data = [0u8; mem::size_of::<AddDevice>()];
                sock.read_exact(&mut add_dev_data).unwrap();
                let add_dev = unsafe {
                    (add_dev_data.as_ptr() as *const AddDevice).as_ref().unwrap()
                };
                let uinput = UInputHandle::new(
                    File::options().write(true).open("/dev/uinput").unwrap()
                );
                for evbit in bitmask_from_slice::<EventKind, _>(&add_dev.evbits).iter() {
                    uinput.set_evbit(evbit).unwrap();
                }
                for keybit in bitmask_from_slice::<Key, _>(&add_dev.keybits).iter() {
                    uinput.set_keybit(keybit).unwrap();
                }
                for relbit in bitmask_from_slice::<RelativeAxis, _>(&add_dev.relbits).iter() {
                    uinput.set_relbit(relbit).unwrap();
                }
                for absbit in bitmask_from_slice::<AbsoluteAxis, _>(&add_dev.absbits).iter() {
                    uinput.set_absbit(absbit).unwrap();
                    let mut absinfo_data = [0u8; mem::size_of::<AbsoluteInfo>()];
                    sock.read_exact(&mut absinfo_data).unwrap();
                    let abs_info = unsafe {
                        (absinfo_data.as_ptr() as *const AbsoluteInfo).as_ref().unwrap()
                    };
                    uinput.abs_setup(&uinput_abs_setup {
                        code: absbit as u16,
                        absinfo: input_absinfo {
                            value: abs_info.value,
                            minimum: abs_info.minimum,
                            maximum: abs_info.maximum,
                            fuzz: abs_info.fuzz,
                            flat: abs_info.flat,
                            resolution: abs_info.resolution,
                        },
                    }).unwrap();
                }
                for mscbit in bitmask_from_slice::<MiscKind, _>(&add_dev.mscbits).iter() {
                    uinput.set_mscbit(mscbit).unwrap();
                }
                for ledbit in bitmask_from_slice::<LedKind, _>(&add_dev.ledbits).iter() {
                    uinput.set_ledbit(ledbit).unwrap();
                }
                for sndbit in bitmask_from_slice::<SoundKind, _>(&add_dev.sndbits).iter() {
                    uinput.set_sndbit(sndbit).unwrap();
                }
                for swbit in bitmask_from_slice::<SwitchKind, _>(&add_dev.swbits).iter() {
                    uinput.set_swbit(swbit).unwrap();
                }
                for propbit in bitmask_from_slice::<InputProperty, _>(&add_dev.propbits).iter() {
                    uinput.set_propbit(propbit).unwrap();
                }
                uinput.dev_setup(&uinput_setup {
                    id: input_id {
                        bustype: add_dev.input_id.bustype,
                        vendor: add_dev.input_id.vendor,
                        product: add_dev.input_id.product,
                        version: add_dev.input_id.version,
                    },
                    name: add_dev.name,
                    ff_effects_max: add_dev.ff_effects,
                }).unwrap();
                uinput.dev_create().unwrap();
                inputs.insert(add_dev.id, uinput);
            },
            REMOVE_DEVICE => {
                let mut remove_dev_data = [0u8; mem::size_of::<RemoveDevice>()];
                sock.read_exact(&mut remove_dev_data).unwrap();
                let remove_dev = unsafe {
                    (remove_dev_data.as_ptr() as *const RemoveDevice).as_ref().unwrap()
                };
                if let Some(uinput) = inputs.remove(&remove_dev.id) {
                    uinput.dev_destroy().unwrap();
                }
            },
            INPUT_EVENT => {
                let mut event_data = [0u8; mem::size_of::<InputEvent>()];
                sock.read_exact(&mut event_data).unwrap();
                let event = unsafe {
                    (event_data.as_ptr() as *const InputEvent).as_ref().unwrap()
                };
                let dev = inputs.get(&event.id);
                if dev.is_none() {
                    continue;
                }
                dev.unwrap().write(&[input_event {
                    time: timeval {
                        tv_sec: event.time_sec,
                        tv_usec: event.time_usec
                    },
                    type_: event.ty,
                    code: event.code,
                    value: event.value,
                }]).unwrap();
            }
            m => panic!("Unknown message {}", m)
        }
    }
}
