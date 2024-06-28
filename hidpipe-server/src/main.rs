use std::{fs::{self, File}, collections::HashMap, mem, env, slice};
use std::collections::hash_map;
use std::ffi::OsStr;
use std::fmt::Debug;
use std::io::{Read, Write, Result};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::fs::OpenOptionsExt;
use udev::{EventType, MonitorBuilder};
use input_linux::{
    evdev::EvdevHandle, InputProperty, EventKind, AbsoluteAxis, Key
};
use libc::{input_event, timeval};
use nix::errno::Errno;
use nix::sys::epoll::{Epoll, EpollCreateFlags, EpollEvent, EpollFlags, EpollTimeout};

use hidpipe_shared::{AddDevice, MessageType, RemoveDevice, ClientHello, ServerHello, InputEvent};

fn is_joystick<F: AsRawFd>(evdev: &EvdevHandle<F>) -> Result<bool> {
    let props = evdev.device_properties()?;
    let no = Ok(false);
    if props.get(InputProperty::Accelerometer) ||
        props.get(InputProperty::PointingStick) ||
        props.get(InputProperty::TopButtonPad) ||
        props.get(InputProperty::ButtonPad) ||
        props.get(InputProperty::SemiMultiTouch) {
        return no;
    }
    let events = evdev.event_bits()?;
    if !events.get(EventKind::Absolute) {
        return no;
    }
    let axes = evdev.absolute_mask()?;
    if !axes.get(AbsoluteAxis::X) || !axes.get(AbsoluteAxis::Y) {
        return no;
    }
    let keys = evdev.key_mask()?;
    return Ok(keys.get(Key::ButtonTrigger) ||
              keys.get(Key::ButtonSouth) ||
              keys.get(Key::Button1) ||
              axes.get(AbsoluteAxis::RX) ||
              axes.get(AbsoluteAxis::RY) ||
              axes.get(AbsoluteAxis::Throttle) ||
              axes.get(AbsoluteAxis::Rudder) ||
              axes.get(AbsoluteAxis::Wheel) ||
              axes.get(AbsoluteAxis::Gas) ||
              axes.get(AbsoluteAxis::Brake)
    );
}

fn send_add_device<F: AsRawFd>(evdev: &EvdevHandle<F>, client: &mut Client) -> Result<()> {
    let abs = evdev.absolute_bits()?;
    let evbits = evdev.event_bits()?.data().clone();
    let keybits = evdev.key_bits()?.data().clone();
    let relbits = evdev.relative_bits()?.data().clone();
    let absbits = abs.data().clone();
    let mscbits = evdev.misc_bits()?.data().clone();
    let ledbits = evdev.led_bits()?.data().clone();
    let sndbits = evdev.sound_bits()?.data().clone();
    let swbits = evdev.switch_bits()?.data().clone();
    let propbits = evdev.device_properties()?.data().clone();
    let input_id = evdev.device_id()?;
    let ff_effects = evdev.effects_count()? as u32;
    let id = evdev.as_raw_fd() as u32;
    let mut name = [0; 80];
    evdev.device_name_buf(&mut name)?;
    client.write(&mut MessageType::AddDevice)?;
    client.write(&mut AddDevice{
        evbits, keybits, relbits, absbits, mscbits, ledbits, id,
        sndbits, swbits, propbits, input_id, name, ff_effects
    })?;
    for bit in abs.iter() {
        let mut info = evdev.absolute_info(bit)?;
        client.write(&mut info)?;
    }
    Ok(())
}

struct EvdevContainer {
    fds_to_devs: HashMap<u64, EvdevHandle<File>>,
    names_to_fds: HashMap<String, u64>
}

fn insert_entry<K, V>(entry: hash_map::Entry<K, V>, v: V) -> &V {
    match entry {
        hash_map::Entry::Vacant(e) => e.insert(v),
        hash_map::Entry::Occupied(mut e) => {
            e.insert(v);
            e.into_mut()
        }
    }
}

impl EvdevContainer {
    fn new() -> EvdevContainer {
        EvdevContainer {
            fds_to_devs: HashMap::new(),
            names_to_fds: HashMap::new()
        }
    }
    fn check_and_add(&mut self, dev_name: &OsStr, file_name: &OsStr, epoll: &Epoll) -> Result<Option<&EvdevHandle<File>>> {
        let dev_name = dev_name.to_string_lossy();
        if !dev_name.starts_with("event") {
            return Ok(None);
        }
        let file = File::options().read(true).write(true).custom_flags(libc::O_NONBLOCK).open(file_name)?;
        let evdev = EvdevHandle::new(file);
        if is_joystick(&evdev)? {
            let raw = evdev.as_raw_fd() as u64;
            self.names_to_fds.insert(dev_name.into_owned(), raw);
            epoll.add(evdev.as_inner(), EpollEvent::new(EpollFlags::EPOLLIN, raw)).unwrap();
            Ok(Some(insert_entry(self.fds_to_devs.entry(raw), evdev)))
        } else {
            Ok(None)
        }
    }
    fn remove(&mut self, dev_name: &OsStr, epoll: &Epoll) -> Option<u32> {
        if let Some(id) = self.names_to_fds.remove(dev_name.to_string_lossy().as_ref()) {
            let id = id as u32;
            let evdev = self.fds_to_devs.remove(&(id as u64)).unwrap();
            epoll.delete(evdev.as_inner()).unwrap();
            Some(id)
        } else {
            None
        }
    }
    fn get(&self, id: u64) -> Option<&EvdevHandle<File>> {
        self.fds_to_devs.get(&id)
    }
    fn iter(&self) -> impl Iterator<Item=&EvdevHandle<File>> {
        self.fds_to_devs.values()
    }
}

struct Client {
    socket: UnixStream,
    buf: Vec<u8>,
    filled: usize,
    ready: bool
}

enum ReadReply {
    Data(Vec<u8>),
    NotReady,
    Hangup
}

impl Client {
    fn new(socket: UnixStream) -> Client {
        Client {
            socket,
            ready: false,
            buf: Vec::new(),
            filled: 0
        }
    }
    fn read(&mut self, size: usize) -> Result<ReadReply> {
        if self.buf.len() == 0 {
            self.buf.resize(size, 0);
        } else if self.buf.len() != size {
            panic!("api misuse");
        }
        let read = self.socket.read(&mut self.buf[self.filled..])?;
        if read == 0 {
            return Ok(ReadReply::Hangup);
        }
        self.filled += read;
        Ok(if self.filled == size {
            let mut ret = Vec::new();
            mem::swap(&mut self.buf, &mut ret);
            self.filled = 0;
            ReadReply::Data(ret)
        } else {
            ReadReply::NotReady
        })
    }
    fn write<T: Debug>(&mut self, data: &mut T) -> Result<()> {
        let size = mem::size_of::<T>();
        // SAFETY:
        // 1. We are taking a ref, so it is valid for reads and properly aligned
        // 2. We are taking a mut ref, so nobody but us can write to it
        let v = unsafe {
            slice::from_raw_parts(data as *mut T as *const u8, size)
        };
        self.socket.write_all(v)
    }
}

fn recv_from_client(clients: &mut HashMap<u64, Client>, epoll: &Epoll, fd: u64, size: usize) -> Option<Vec<u8>> {
    let client = clients.get_mut(&fd).unwrap();
    match client.read(size) {
        Ok(ReadReply::NotReady) => None,
        Ok(ReadReply::Data(data)) => Some(data),
        Ok(ReadReply::Hangup) => {
            epoll.delete(&client.socket).unwrap();
            clients.remove(&fd);
            None
        },
        Err(e) => {
            eprintln!("Client {} disconnected with error: {:?}", fd, e);
            epoll.delete(&client.socket).unwrap();
            clients.remove(&fd);
            None
        }
    }
}

fn hangup_on_error_bcast<F>(clients: &mut HashMap<u64, Client>, epoll: &Epoll, mut f: F) where F: FnMut(&mut Client) -> Result<()> {
    clients.retain(|k, v| {
        let res = f(v);
        if res.is_err() {
            eprintln!("Client {} disconnected with error: {:?}", *k, res.unwrap_err());
            epoll.delete(&v.socket).unwrap();
            false
        } else {
            true
        }
    });
}

fn hangup_on_error<F>(clients: &mut HashMap<u64, Client>, epoll: &Epoll, fd: u64, f: F) where F: FnOnce(&mut Client) -> Result<()> {
    let client = clients.get_mut(&fd).unwrap();
    let res = f(client);
    if res.is_err() {
        eprintln!("Client {} disconnected with error: {:?}", fd, res.unwrap_err());
        epoll.delete(&client.socket).unwrap();
        clients.remove(&fd);
    }
}

fn main() {
    let udev_socket = MonitorBuilder::new().unwrap()
        .match_subsystem("input").unwrap()
        .listen().unwrap();
    let mut evdevs = EvdevContainer::new();
    let mut clients = HashMap::new();
    let epoll = Epoll::new(EpollCreateFlags::empty()).unwrap();
    for dir_ent in fs::read_dir("/dev/input/").unwrap() {
        let dir_ent = dir_ent.unwrap();
        let name = dir_ent.file_name();
        let res = evdevs.check_and_add(&name, dir_ent.path().as_os_str(), &epoll);
        if let Err(e) = res {
            eprintln!("Unable to determine if {} is a joystick, error: {:?}", name.to_string_lossy(), e);
        }
    }
    epoll.add(&udev_socket, EpollEvent::new(EpollFlags::EPOLLIN, udev_socket.as_raw_fd() as u64)).unwrap();
    let xdg_dir = env::var("XDG_RUNTIME_DIR");
    if xdg_dir.is_err() {
        eprintln!("Unable to get XDG_RUNTIME_DIR, error: {:?}", xdg_dir.unwrap_err());
        return;
    }
    let sock_path = format!("{}/hidpipe", xdg_dir.unwrap());
    _ = fs::remove_file(&sock_path);
    let listen_sock = UnixListener::bind(sock_path).unwrap();
    epoll.add(&listen_sock, EpollEvent::new(EpollFlags::EPOLLIN, listen_sock.as_raw_fd() as u64)).unwrap();

    loop {
        let mut evts = [EpollEvent::empty()];
        match epoll.wait(&mut evts, EpollTimeout::NONE) {
            Err(Errno::EINTR) | Ok(0) => {
                continue;
            },
            Ok(_) => {},
            e => {
                e.unwrap();
            },
        }
        let fd = evts[0].data();
        if fd == udev_socket.as_raw_fd() as u64 {
            for event in udev_socket.iter() {
                match event.event_type() {
                    EventType::Remove => {
                        if let Some(id) = evdevs.remove(event.sysname(), &epoll) {
                            hangup_on_error_bcast(&mut clients, &epoll, |client| {
                                client.write(&mut MessageType::RemoveDevice)?;
                                client.write(&mut RemoveDevice{id})
                            });
                        }
                    },
                    EventType::Add => {
                        let name = event.sysname();
                        let node = event.devnode();
                        if node.is_none() {
                            continue;
                        }
                        let res = evdevs.check_and_add(&name, node.unwrap().as_os_str(), &epoll);
                        match res {
                            Err(e) => {
                                eprintln!("Unable to determine if {} is a joystick, error: {:?}", name.to_string_lossy(), e);
                            },
                            Ok(None) => {},
                            Ok(Some(dev)) => {
                                hangup_on_error_bcast(&mut clients, &epoll, |client| {
                                    send_add_device(dev, client)
                                });
                            }
                        }
                    },
                    _ => {
                    }
                }
            }
        } else if fd == listen_sock.as_raw_fd() as u64 {
            let res = listen_sock.accept();
            if res.is_err() {
                eprintln!("Failed to accept a connection, error: {:?}", res.unwrap_err());
                continue;
            }
            let stream = res.unwrap().0;
            stream.set_nonblocking(true).unwrap();
            let raw = stream.as_raw_fd() as u64;
            epoll.add(&stream, EpollEvent::new(EpollFlags::EPOLLIN, raw)).unwrap();
            let client = Client::new(stream);
            clients.insert(raw, client);
        } else if let Some(client) = clients.get(&fd) {
            if client.ready {
                continue
            }
            let data = recv_from_client(&mut clients, &epoll, fd, mem::size_of::<ClientHello>());
            if data.is_none() {
                continue
            }
            hangup_on_error(&mut clients, &epoll, fd, |client| {
                client.write(&mut ServerHello {
                    version: 0
                })?;
                for dev in evdevs.iter() {
                    send_add_device(dev, client)?;
                }
                client.ready = true;
                Ok(())
            });
        } else if let Some(evdev) = evdevs.get(fd) {
            let evt = input_event {
                time: timeval { tv_sec: 0, tv_usec: 0 },
                type_: 0,
                code: 0,
                value: 0,
            };
            let mut evts = [evt];
            while let Ok(count) = evdev.read(&mut evts) {
                if count == 0 {
                    break;
                }
                let mut ev = InputEvent::new(fd as u32, evts[0].into());
                hangup_on_error_bcast(&mut clients, &epoll, |client| {
                    if !client.ready {
                        return Ok(());
                    }
                    client.write(&mut MessageType::InputEvent)?;
                    client.write(&mut ev)
                });
            }
        }
    }
}
