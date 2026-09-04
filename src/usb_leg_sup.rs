use bitbybit::bitfield;

#[bitfield(u32, debug)]
pub struct UsbLegSupReg {
    #[bit(16, r)]
    pub bios_owned_semaphore: bool,
    #[bit(24, rw)]
    pub os_owned_semaphor: bool,
}

#[bitfield(u32, debug)]
pub struct UsbLegCtlStsReg {
    #[bit(0, rw)]
    pub usb_smi_enable: bool,
    #[bit(1, rw)]
    pub smi_on_usb_error_enable: bool,
    #[bit(2, rw)]
    pub smi_on_port_change_enable: bool,
    #[bit(3, rw)]
    pub smi_on_frame_list_rollover_enable: bool,
    #[bit(4, rw)]
    pub smi_on_host_system_error_enable: bool,
    #[bit(5, rw)]
    pub smi_on_async_event_enable: bool,
    #[bit(13, rw)]
    pub smi_on_os_ownership_enable: bool,
    #[bit(14, rw)]
    pub smi_on_pci_command_enable: bool,
    #[bit(15, rw)]
    pub smi_on_bar_enable: bool,
    #[bit(16, r)]
    pub smi_on_usb_complete: bool,
    #[bit(17, r)]
    pub smi_on_usb_error: bool,
    #[bit(18, r)]
    pub smi_on_port_change_detect: bool,
    #[bit(19, r)]
    pub smi_on_frame_list_rollover: bool,
    #[bit(20, r)]
    pub smi_on_host_system_error: bool,
    #[bit(21, r)]
    pub smi_on_async_advance: bool,
    #[bit(29, rw)]
    pub smi_on_os_ownership: bool,
    #[bit(30, rw)]
    pub smi_on_pci_command: bool,
    #[bit(31, rw)]
    pub smi_on_bar: bool,
}
