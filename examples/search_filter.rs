// Finding components with search filters.
//
// The getter methods for a device's tree -- channels, signals, devices,
// function blocks, and a folder's items -- all come in a `_with` variant that
// takes an optional search filter.  A filter answers two questions about
// every component the search walks over: "accept this one into the result?"
// and "descend into its children?".
//
// With no filter the getters return only the immediate, visible children, so
// `channels()` of the top-level device below finds nothing -- the channels
// live one device deeper.  A `SearchFilter::recursive` wrapper makes the
// search descend.
//
// Filters compose by construction: `SearchFilter::and`, `SearchFilter::or`,
// and `SearchFilter::not` take other filters as their arguments.  The
// recursive wrapper must be the outermost one -- never nest it inside
// another filter.

use opendaq::{Component, Instance, Interface, SearchFilter, TagsPrivate};

/// Print `components` by their local id under `label`.
fn show<T: Interface>(label: &str, components: Vec<T>) -> opendaq::Result<()> {
    let ids = components
        .iter()
        .map(|c| c.to_base_object().cast::<Component>()?.local_id())
        .collect::<opendaq::Result<Vec<_>>>()?;
    let listing = if ids.is_empty() {
        "(none)".to_string()
    } else {
        ids.join(", ")
    };
    println!("{label}\n    => {listing}\n");
    Ok(())
}

fn main() -> opendaq::Result<()> {
    let instance = Instance::new()?;
    instance.add_device("daqref://device0")?;
    let root = instance.root_device()?.expect("root device");

    // Give a couple of channels some tags, so the tag filter has something to
    // match.  (RefCh0 and RefCh1 come from the reference device unlabelled.)
    let everything = SearchFilter::recursive(&SearchFilter::any()?)?;
    for channel in root.channels_with(Some(&everything))? {
        let tags = channel.tags()?.expect("tags").cast::<TagsPrivate>()?;
        tags.add("analog")?;
        if channel.local_id()? == "RefCh0" {
            tags.add("primary")?;
        }
    }

    show(
        "channels, no filter (immediate children only)",
        root.channels()?,
    )?;

    show(
        "channels, recursive(any)",
        root.channels_with(Some(&SearchFilter::recursive(&SearchFilter::any()?)?))?,
    )?;

    show(
        "channels, recursive(id = RefCh1)",
        root.channels_with(Some(&SearchFilter::recursive(&SearchFilter::local_id(
            "RefCh1",
        )?)?))?,
    )?;

    let ai0_or_ai1 = SearchFilter::or(
        &SearchFilter::local_id("AI0")?,
        &SearchFilter::local_id("AI1")?,
    )?;
    show(
        "signals, recursive(id = AI0 OR id = AI1)",
        root.signals_with(Some(&SearchFilter::recursive(&ai0_or_ai1)?))?,
    )?;

    let not_ref_ch1 = SearchFilter::not(&SearchFilter::local_id("RefCh1")?)?;
    show(
        "channels, recursive(NOT id = RefCh1)",
        root.channels_with(Some(&SearchFilter::recursive(&not_ref_ch1)?))?,
    )?;

    let analog_and_not_ref_ch1 = SearchFilter::and(
        &SearchFilter::required_tags(vec!["analog"])?,
        &SearchFilter::not(&SearchFilter::local_id("RefCh1")?)?,
    )?;
    show(
        "channels, recursive(tag = analog AND NOT id = RefCh1)",
        root.channels_with(Some(&SearchFilter::recursive(&analog_and_not_ref_ch1)?))?,
    )?;

    Ok(())
}
