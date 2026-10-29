pub fn tune_udp_socket(sock: &socket2::Socket) {
    const BUFFER_SIZE: usize = 8 * 1024 * 1024; // 8 MB
    let _ = sock.set_recv_buffer_size(BUFFER_SIZE);
    let _ = sock.set_send_buffer_size(BUFFER_SIZE);
    let _ = sock.set_nonblocking(true);
    disable_udp_connreset_raw(sock);
}

#[cfg(not(windows))]
pub fn prevent_windows_background_throttling() {
    // Non-Windows systems do not require timer resolution or EcoQoS workarounds
}

#[cfg(not(windows))]
pub fn disable_udp_connreset_raw(_sock: &socket2::Socket) {
    // SIO_UDP_CONNRESET is Windows-specific
}

#[cfg(windows)]
pub fn prevent_windows_background_throttling() {
    use windows_sys::Win32::Media::timeBeginPeriod;
    use windows_sys::Win32::System::Power::{
        SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, ProcessPowerThrottling, SetProcessInformation,
        PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE,
    };

    unsafe {
        // High-resolution 1ms timer
        timeBeginPeriod(1);

        // Prevent thread/system sleep while active
        SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED);

        // Turn off EcoQoS / background power throttling
        let mut throttling = PROCESS_POWER_THROTTLING_STATE {
            Version: 1,
            ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
            StateMask: 0,
        };
        SetProcessInformation(
            GetCurrentProcess(),
            ProcessPowerThrottling,
            &mut throttling as *mut _ as *mut _,
            std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        );
    }
}

#[cfg(windows)]
pub fn disable_udp_connreset_raw(sock: &socket2::Socket) {
    use std::os::windows::io::AsRawSocket;
    use windows_sys::Win32::Networking::WinSock::{WSAIoctl, SOCKET};

    const SIO_UDP_CONNRESET: u32 = 0x9800000C;
    let raw_socket = sock.as_raw_socket() as SOCKET;
    let mut dw_bytes_returned: u32 = 0;
    let mut b_new_behavior: u32 = 0;

    unsafe {
        WSAIoctl(
            raw_socket,
            SIO_UDP_CONNRESET,
            &mut b_new_behavior as *mut _ as *mut _,
            std::mem::size_of::<u32>() as u32,
            std::ptr::null_mut(),
            0,
            &mut dw_bytes_returned,
            std::ptr::null_mut(),
            None,
        );
    }
}
