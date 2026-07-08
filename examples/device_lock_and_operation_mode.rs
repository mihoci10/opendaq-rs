// Switch a device between its operation modes and lock it against changes.

use opendaq::{Instance, OperationModeType};

fn main() -> opendaq::Result<()> {
    let instance = Instance::new()?;
    let device = instance
        .add_device("daqref://device0")?
        .expect("reference device");

    let modes: Vec<String> = device
        .available_operation_modes()?
        .into_iter()
        .filter_map(|mode| OperationModeType::from_raw(mode as u32))
        .map(|mode| format!("{mode:?}"))
        .collect();
    println!("Available operation modes: {}", modes.join(", "));
    println!("Current operation mode:    {:?}", device.operation_mode()?);

    device.set_operation_mode(OperationModeType::Operation)?;
    device.lock()?;
    println!("After setting Operation:   {:?}", device.operation_mode()?);
    println!("Device locked: {}", device.is_locked()?);

    device.unlock()?;
    device.set_operation_mode(OperationModeType::SafeOperation)?;
    println!();
    println!("Device locked: {}", device.is_locked()?);
    println!("Final operation mode:      {:?}", device.operation_mode()?);
    Ok(())
}
