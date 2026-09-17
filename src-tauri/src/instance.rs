//! Objets nommés Windows : verrou d'instance et signal de réactivation.
use windows_sys::Win32::{
    Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, WAIT_OBJECT_0},
    System::Threading::{CreateEventW, CreateMutexW, SetEvent, WaitForSingleObject},
};

pub struct Instance {
    mutex: HANDLE,
    event: HANDLE,
}
impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.mutex);
            CloseHandle(self.event);
        }
    }
}
fn wide(name: &str) -> Vec<u16> {
    name.encode_utf16().chain(Some(0)).collect()
}
const MUTEX: &str = "Local\\io.github.a8naaijetvi2lunk.vigie.instance";
const EVENT: &str = "Local\\io.github.a8naaijetvi2lunk.vigie.instance.activate";

impl Instance {
    pub fn acquire() -> std::io::Result<Option<Self>> {
        Self::acquire_named(MUTEX)
    }
    fn acquire_named(name: &str) -> std::io::Result<Option<Self>> {
        // L'événement existe avant le verrou : un second lancement pendant le
        // démarrage laisse son signal en attente jusqu'à la création des fenêtres.
        let event_name = wide(&format!("{name}.activate"));
        let event = unsafe { CreateEventW(std::ptr::null(), 0, 0, event_name.as_ptr()) };
        if event.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let name = wide(name);
        // SECURITY_ATTRIBUTES null : ACL par défaut de l'utilisateur, pas d'héritage.
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        if handle.is_null() {
            let error = std::io::Error::last_os_error();
            unsafe {
                CloseHandle(event);
            }
            return Err(error);
        }
        let exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        let instance = Self {
            mutex: handle,
            event,
        };
        if exists {
            drop(instance);
            Ok(None)
        } else {
            Ok(Some(instance))
        }
    }
}

pub fn activate_existing() {
    let name = wide(EVENT);
    unsafe {
        let event = CreateEventW(std::ptr::null(), 0, 0, name.as_ptr());
        if !event.is_null() {
            SetEvent(event);
            CloseHandle(event);
        }
    }
}

pub fn listen(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        let name = wide(EVENT);
        unsafe {
            let event = CreateEventW(std::ptr::null(), 0, 0, name.as_ptr());
            if event.is_null() {
                return;
            }
            loop {
                match WaitForSingleObject(event, u32::MAX) {
                    WAIT_OBJECT_0 => crate::show_sessions(&app),
                    _ => break,
                }
            }
            CloseHandle(event);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_one_handle_can_be_the_first_instance() {
        let name = format!("Local\\vigie-test-{}", std::process::id());
        let first = Instance::acquire_named(&name).unwrap().unwrap();
        assert!(Instance::acquire_named(&name).unwrap().is_none());
        drop(first);
        assert!(Instance::acquire_named(&name).unwrap().is_some());
    }
    #[test]
    fn activation_waits_for_the_listener() {
        let name = format!("Local\\vigie-event-test-{}", std::process::id());
        let first = Instance::acquire_named(&name).unwrap().unwrap();
        unsafe {
            assert_ne!(SetEvent(first.event), 0);
            assert_eq!(WaitForSingleObject(first.event, 0), WAIT_OBJECT_0);
            assert_ne!(WaitForSingleObject(first.event, 0), WAIT_OBJECT_0);
        }
    }
}
