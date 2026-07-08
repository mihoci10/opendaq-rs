//! Property conveniences: batched updates and component-kind discovery.

use crate::error::Result;
use crate::generated::PropertyObject;
use crate::object::BaseObject;

/// Batches property writes into one update per bracketed object (RAII over
/// `beginUpdate` / `endUpdate`).
///
/// While the guard is alive, property values set on the objects -- or on any
/// of their descendant property objects -- are staged rather than applied:
/// reads still return the old values and no `OnPropertyValueWrite` /
/// `OnEndUpdate` events fire.  Dropping the guard commits each object's
/// staged values at once, in reverse order.
///
/// ```no_run
/// # fn main() -> opendaq::Result<()> {
/// # let (ch0, ch1): (opendaq::Channel, opendaq::Channel) = todo!();
/// let batch = opendaq::BatchedPropertyUpdate::new(&[&ch0, &ch1])?;
/// ch0.set_property_value("Amplitude", 2.5)?;
/// ch1.set_property_value("Amplitude", 4.0)?;
/// batch.commit()?; // or just drop the guard
/// # Ok(()) }
/// ```
#[must_use = "dropping the guard immediately would commit the batch at once"]
pub struct BatchedPropertyUpdate {
    objects: Vec<PropertyObject>,
}

impl BatchedPropertyUpdate {
    /// Open one update batch per object (accepts any property-object-derived
    /// wrapper: devices, channels, function blocks, ...).
    pub fn new(objects: &[&PropertyObject]) -> Result<BatchedPropertyUpdate> {
        let mut opened: Vec<PropertyObject> = Vec::with_capacity(objects.len());
        for object in objects {
            if let Err(e) = object.begin_update() {
                // Unwind the batches already opened before reporting.
                for open in opened.iter().rev() {
                    let _ = open.end_update();
                }
                return Err(e);
            }
            opened.push((*object).clone());
        }
        Ok(BatchedPropertyUpdate { objects: opened })
    }

    /// Commit the batch now, surfacing any `endUpdate` error (dropping the
    /// guard commits too, but swallows errors).
    pub fn commit(mut self) -> Result<()> {
        let objects = std::mem::take(&mut self.objects);
        std::mem::forget(self);
        for object in objects.iter().rev() {
            object.end_update()?;
        }
        Ok(())
    }
}

impl Drop for BatchedPropertyUpdate {
    fn drop(&mut self) {
        for object in self.objects.iter().rev() {
            let _ = object.end_update();
        }
    }
}

/// The most-derived component interface an object implements, letting you
/// discover a component's concrete type before committing to a cast.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ComponentKind {
    Channel,
    FunctionBlock,
    Device,
    Signal,
    InputPort,
    Folder,
    Component,
}

impl BaseObject {
    /// The most-derived component interface this object implements (probed
    /// most-derived first), or `None` when it is not a component.
    pub fn component_kind(&self) -> Option<ComponentKind> {
        if self.is_a::<crate::Channel>() {
            Some(ComponentKind::Channel)
        } else if self.is_a::<crate::FunctionBlock>() {
            Some(ComponentKind::FunctionBlock)
        } else if self.is_a::<crate::Device>() {
            Some(ComponentKind::Device)
        } else if self.is_a::<crate::Signal>() {
            Some(ComponentKind::Signal)
        } else if self.is_a::<crate::InputPort>() {
            Some(ComponentKind::InputPort)
        } else if self.is_a::<crate::Folder>() {
            Some(ComponentKind::Folder)
        } else if self.is_a::<crate::Component>() {
            Some(ComponentKind::Component)
        } else {
            None
        }
    }
}
