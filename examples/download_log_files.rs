// Download a device's log files: enumerate them with `Device::log_file_infos`
// and pull each one's contents with `Device::log`.  On a real (often remote)
// device the logs already exist; the bundled reference device only produces
// one when you ask it to, so this example first sets that up:
//
//   * give the instance a file logger sink, so openDAQ actually writes a log
//     file;
//   * add the device with EnableLogging = true and LoggingPath pointing at
//     that same file -- that is the file the reference device reports and
//     serves.
//
// `log_file_infos` / `log` then behave exactly as they would against a remote
// device.

use std::fs;

use opendaq::{Instance, InstanceBuilder, LoggerSink, Property, PropertyObject};

fn main() -> opendaq::Result<()> {
    let work_dir = std::env::temp_dir().join("opendaq-rs-log-example");
    let downloads = work_dir.join("downloads");
    fs::create_dir_all(&downloads).expect("create working directories");
    let device_log = work_dir.join("ref_device_simulator.log");
    let device_log_path = device_log.to_string_lossy().into_owned();

    // Start from a clean log so the reported size reflects only this run.
    let _ = fs::remove_file(&device_log);

    let builder = InstanceBuilder::new()?;
    // Find the bundled modules.
    let modules = opendaq::native_library_directory().expect("bundled native modules");
    builder.set_module_path(&modules.to_string_lossy())?;
    builder.add_logger_sink(&LoggerSink::basic_file(&device_log_path)?)?;
    let instance = Instance::from_builder(&builder)?;

    // The reference device reads these two properties from its add-device config.
    let config = PropertyObject::new()?;
    config.add_property(&Property::bool("EnableLogging", true, true)?)?;
    config.add_property(&Property::string("LoggingPath", &device_log_path, true)?)?;

    let device = instance
        .add_device_with("daqref://device0", Some(&config))?
        .expect("reference device");

    // Flush the logger so everything buffered so far is actually on disk
    // before we read it.
    let context = instance.context()?.expect("instance context");
    context.logger()?.expect("instance logger").flush()?;

    let infos = device.log_file_infos()?;
    if infos.is_empty() {
        println!("Device exposes no log files.");
        return Ok(());
    }
    println!("Device exposes {} log file(s):", infos.len());
    println!();
    for info in infos {
        let id = info.id()?;
        let destination = downloads.join(info.name()?);
        println!("- {}", info.name()?);
        println!("    id:            {id}");
        println!("    size:          {} bytes", info.size()?);
        println!("    encoding:      {}", info.encoding()?);
        println!("    last-modified: {}", info.last_modified()?);
        // Size/offset let you fetch just part of a file; here, a short head preview.
        println!("    preview:       {:?}", device.log_with(&id, 60, 0)?);
        // Plain `log` fetches the whole file in one call.
        let content = device.log(&id)?;
        fs::write(&destination, &content).expect("write the downloaded log");
        println!(
            "    downloaded {} chars -> {}",
            content.len(),
            destination.display()
        );
        println!();
    }
    Ok(())
}
