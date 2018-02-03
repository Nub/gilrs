// Copyright 2017 Mateusz Sieczko and other GilRs Developers
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use AsInner;
use ev::{Axis, AxisOrBtn, Button, Code, Event, EventType, RawEvent, RawEventType};
use ev::state::{AxisData, ButtonData, GamepadState};
use ff::Error as FfError;
use ff::server::{self, Message};
use mapping::{Mapping, MappingData, MappingDb, MappingError};
use platform;

use uuid::Uuid;

use std::collections::VecDeque;
use std::ops::{Index, IndexMut};
use std::sync::mpsc::Sender;

/// Main object responsible of managing gamepads.
///
/// # Event loop
///
/// All interesting actions like button was pressed or new controller was connected are represented
/// by struct [`Event`](struct.Event.html). Use `next_event()` function to retrieve event from
/// queue.
///
/// ```
/// use gilrs::{Gilrs, Event, EventType, Button};
///
/// let mut gilrs = Gilrs::new();
///
/// // Event loop
/// loop {
///     while let Some(event) = gilrs.next_event() {
///         match event {
///             Event { id, event: EventType::ButtonPressed(Button::South, _), .. } => {
///                 println!("Player {}: jump!", id + 1)
///             }
///             Event { id, event: EventType::Disconnected, .. } => {
///                 println!("We lost player {}", id + 1)
///             }
///             _ => (),
///         };
///     }
///     # break;
/// }
/// ```
///
/// # Cached gamepad state
///
/// `Gilrs` also menage cached gamepad state. To update it, use `update(Event)` method. Updating is
/// not done automatically, because you probably want the state after filtered events (see
/// [`ev::filter`](ev/filter/index.html) module), not these from `event_next()`.
///
/// To access state you can use `Gamepad::state()` function. Gamepad also implement some state
/// related functions directly, see [`Gamepad`](struct.Gamepad.html) for more.
///
/// ## Counter
///
/// `Gilrs` has additional functionality, referred here as *counter*. The idea behind it is simple,
/// each time you end iteration of update loop, you call `Gilrs::inc()` which will increase
/// internal counter by one. When state of one if elements changes, value of counter is saved. When
/// checking state of one of elements you can tell exactly when this event happened. Timestamps are
/// not good solution here because they can tell you when *system* observed event, not when you
/// processed it. On the other hand, they are good when you want to implement key repeat or software
/// debouncing.
///
/// ```
/// use gilrs::{Gilrs, Button};
///
/// let mut gilrs = Gilrs::new();
///
/// loop {
///     while let Some(ev) = gilrs.next_event() {
///         gilrs.update(&ev);
///         // Do other things with event
///     }
///
///     if gilrs.gamepad(0).is_pressed(Button::DPadLeft) {
///         // go left
///     }
///
///     match gilrs.gamepad(0).button_data(Button::South) {
///         Some(d) if d.is_pressed() && d.counter() == gilrs.counter() => {
///             // jump
///         }
///         _ => ()
///     }
///
///     gilrs.inc();
/// #   break;
/// }
///
#[derive(Debug)]
pub struct Gilrs {
    inner: platform::Gilrs,
    next_id: usize,
    tx: Sender<Message>,
    counter: u64,
    mappings: MappingDb,
    default_filters: bool,
    events: VecDeque<Event>,
    axis_to_btn_pressed: f32,
    axis_to_btn_released: f32,
}

impl Gilrs {
    /// Creates new `Gilrs` with default settings. See [`GilrsBuilder`](struct.GilrsBuilder.html)
    /// for more details.
    pub fn new() -> Self {
        GilrsBuilder::new().add_included_mappings().build()
    }

    /// Returns next pending event.
    pub fn next_event(&mut self) -> Option<Event> {
        use ev::filter::{axis_dpad_to_button, deadzone, Filter, Jitter};

        if self.default_filters {
            let jitter_filter = Jitter::new();
            loop {
                let ev = self.next_event_priv()
                    .filter_ev(&axis_dpad_to_button, self)
                    .filter_ev(&jitter_filter, self)
                    .filter_ev(&deadzone, self);

                // Skip all dropped events, there is no reason to return them
                match ev {
                    Some(ev) if ev.is_dropped() => (),
                    _ => break ev,
                }
            }
        } else {
            self.next_event_priv()
        }
    }

    /// Returns next pending event.
    fn next_event_priv(&mut self) -> Option<Event> {
        if let Some(ev) = self.events.pop_front() {
            Some(ev)
        } else {
            match self.inner.next_event() {
                Some(RawEvent { id, event, time }) => {
                    debug!("Original event: {:?}", RawEvent { id, event, time });
                    let gamepad = self.inner.gamepad_mut(id);
                    let event = match event {
                        RawEventType::ButtonPressed(nec) => {
                            let nec = Code(nec);
                            match gamepad.axis_or_btn_name(nec) {
                                Some(AxisOrBtn::Btn(b)) => {
                                    self.events.push_back(Event {
                                        id,
                                        time,
                                        event: EventType::ButtonChanged(b, 1.0, nec),
                                    });

                                    EventType::ButtonPressed(b, nec)
                                }
                                Some(AxisOrBtn::Axis(a)) => EventType::AxisChanged(a, 1.0, nec),
                                None => {
                                    self.events.push_back(Event {
                                        id,
                                        time,
                                        event: EventType::ButtonChanged(Button::Unknown, 1.0, nec),
                                    });

                                    EventType::ButtonPressed(Button::Unknown, nec)
                                }
                            }
                        }
                        RawEventType::ButtonReleased(nec) => {
                            let nec = Code(nec);
                            match gamepad.axis_or_btn_name(nec) {
                                Some(AxisOrBtn::Btn(b)) => {
                                    self.events.push_back(Event {
                                        id,
                                        time,
                                        event: EventType::ButtonChanged(b, 0.0, nec),
                                    });

                                    EventType::ButtonReleased(b, nec)
                                }
                                Some(AxisOrBtn::Axis(a)) => EventType::AxisChanged(a, 0.0, nec),
                                None => {
                                    self.events.push_back(Event {
                                        id,
                                        time,
                                        event: EventType::ButtonChanged(Button::Unknown, 0.0, nec),
                                    });

                                    EventType::ButtonReleased(Button::Unknown, nec)
                                }
                            }
                        }
                        RawEventType::AxisValueChanged(val, nec) => {
                            // Let's trust at least our backend code
                            let axis_info = gamepad.inner.axis_info(nec).unwrap();
                            let nec = Code(nec);

                            match gamepad.axis_or_btn_name(nec) {
                                Some(AxisOrBtn::Btn(b)) => {
                                    let val = axis_info.value_btn(val);

                                    if val >= self.axis_to_btn_pressed
                                        && !gamepad.state().is_pressed(&nec)
                                    {
                                        self.events.push_back(Event {
                                            id,
                                            time,
                                            event: EventType::ButtonChanged(b, val, nec),
                                        });

                                        EventType::ButtonPressed(b, nec)
                                    } else if val <= self.axis_to_btn_released
                                        && gamepad.state().is_pressed(&nec)
                                    {
                                        self.events.push_back(Event {
                                            id,
                                            time,
                                            event: EventType::ButtonChanged(b, val, nec),
                                        });

                                        EventType::ButtonReleased(b, nec)
                                    } else {
                                        EventType::ButtonChanged(b, val, nec)
                                    }
                                }
                                Some(AxisOrBtn::Axis(a)) => {
                                    EventType::AxisChanged(a, axis_info.value_axis(val), nec)
                                }
                                None => EventType::AxisChanged(
                                    Axis::Unknown,
                                    axis_info.value_axis(val),
                                    nec,
                                ),
                            }
                        }
                        RawEventType::Connected => {
                            gamepad.status = Status::Connected;
                            let mapping = self.mappings
                                .get(gamepad.uuid())
                                .and_then(|s| {
                                    Mapping::parse_sdl_mapping(
                                        s,
                                        gamepad.inner.buttons(),
                                        gamepad.inner.axes(),
                                    ).ok()
                                })
                                .unwrap_or_default();
                            gamepad.mapping = mapping;

                            if gamepad.id == usize::max_value() {
                                gamepad.id = id;
                                gamepad.tx = self.tx.clone();

                                if let Some(device) = gamepad.inner.ff_device() {
                                    let _ = self.tx.send(Message::Open { id, device });
                                }
                            }

                            EventType::Connected
                        }
                        RawEventType::Disconnected => {
                            gamepad.status = Status::Disconnected;
                            let _ = self.tx.send(Message::Close { id });

                            EventType::Disconnected
                        }
                    };

                    Some(Event { id, event, time })
                }
                None => None,
            }
        }
    }

    /// Updates internal state according to `event`.
    pub fn update(&mut self, event: &Event) {
        use EventType::*;

        let counter = self.counter;

        let gamepad = match self.connected_gamepad_mut(event.id) {
            Some(g) => g,
            None => return,
        };

        match event.event {
            ButtonPressed(_, nec) => {
                gamepad
                    .state
                    .set_btn_pressed(nec, true, counter, event.time);
            }
            ButtonReleased(_, nec) => {
                gamepad
                    .state
                    .set_btn_pressed(nec, false, counter, event.time);
            }
            ButtonRepeated(_, nec) => {
                gamepad.state.set_btn_repeating(nec, counter, event.time);
            }
            ButtonChanged(_, value, nec) => {
                gamepad.state.set_btn_value(nec, value, counter, event.time);
            }
            AxisChanged(_, value, nec) => {
                gamepad
                    .state
                    .update_axis(nec, AxisData::new(value, counter, event.time));
            }
            _ => (),
        }
    }

    /// Increases internal counter by one. Counter data is stored with state and can be used to
    /// determine when last event happened. You probably want to use this function in your update
    /// loop after processing events.
    pub fn inc(&mut self) {
        // Counter is 62bit. See `ButtonData`.
        if self.counter == 0x3FFF_FFFF_FFFF_FFFF {
            self.counter = 0;
        } else {
            self.counter += 1;
        }
    }

    /// Returns counter. Counter data is stored with state and can be used to determine when last
    /// event happened.
    pub fn counter(&self) -> u64 {
        self.counter
    }

    /// Sets counter to 0.
    pub fn reset_counter(&mut self) {
        self.counter = 0;
    }

    fn create_ff_devices(&self) {
        for (id, gp) in self.gamepads()
            .filter(|&(_, g)| g.is_ff_supported())
            .map(|(id, g)| (id, g.inner.ff_device()))
        {
            if let Some(device) = gp {
                let _ = self.tx.send(Message::Open { id, device });
            }
        }
    }

    fn finish_gamepads_creation(&mut self) {
        let tx = self.tx.clone();
        for (id, gp) in self.gamepads_mut() {
            gp.id = id;
            gp.tx = tx.clone();
        }
    }

    /// Borrow gamepad with given id. This method always return reference to some gamepad, even if
    /// it was disconnected or never observed. If gamepad's status is not equal to
    /// `Status::Connected` all actions preformed on it are no-op and all values in cached gamepad
    /// state are 0 (false for buttons and 0.0 for axes).
    pub fn gamepad(&self, id: usize) -> &Gamepad {
        self.inner.gamepad(id)
    }

    /// See `gamepad()`
    pub fn gamepad_mut(&mut self, id: usize) -> &mut Gamepad {
        self.inner.gamepad_mut(id)
    }

    /// Returns iterator over all connected gamepads and their ids.
    ///
    /// ```
    /// # let gilrs = gilrs::Gilrs::new();
    /// for (id, gamepad) in gilrs.gamepads() {
    ///     assert!(gamepad.is_connected());
    ///     println!("Gamepad with id {} and name {} is connected",
    ///              id, gamepad.name());
    /// }
    /// ```
    pub fn gamepads(&self) -> ConnectedGamepadsIterator {
        ConnectedGamepadsIterator(self, 0)
    }

    /// Returns iterator over all connected gamepads and their ids.
    ///
    /// ```
    /// # let mut gilrs = gilrs::Gilrs::new();
    /// for (id, gamepad) in gilrs.gamepads_mut() {
    ///     assert!(gamepad.is_connected());
    ///     println!("Gamepad with id {} and name {} is connected",
    ///              id, gamepad.name());
    /// }
    /// ```
    pub fn gamepads_mut(&mut self) -> ConnectedGamepadsMutIterator {
        ConnectedGamepadsMutIterator(self, 0)
    }

    /// Returns a reference to connected gamepad or `None`.
    pub fn connected_gamepad(&self, id: usize) -> Option<&Gamepad> {
        let gp = self.inner.gamepad(id);
        if gp.is_connected() {
            Some(gp)
        } else {
            None
        }
    }

    /// Returns a mutable reference to connected gamepad or `None`.
    pub fn connected_gamepad_mut(&mut self, id: usize) -> Option<&mut Gamepad> {
        let gp = self.inner.gamepad_mut(id);
        if gp.is_connected() {
            Some(gp)
        } else {
            None
        }
    }

    pub(crate) fn ff_sender(&self) -> &Sender<Message> {
        &self.tx
    }

    pub(crate) fn next_ff_id(&mut self) -> usize {
        // TODO: reuse free ids
        let id = self.next_id;
        self.next_id = match self.next_id.checked_add(1) {
            Some(x) => x,
            None => panic!("Failed to assign ID to new effect"),
        };
        id
    }
}

impl Index<usize> for Gilrs {
    type Output = Gamepad;

    fn index(&self, idx: usize) -> &Gamepad {
        self.gamepad(idx)
    }
}

impl IndexMut<usize> for Gilrs {
    fn index_mut(&mut self, idx: usize) -> &mut Gamepad {
        self.gamepad_mut(idx)
    }
}

/// Allow to create `Gilrs ` with customized behaviour.
pub struct GilrsBuilder {
    mappings: MappingDb,
    default_filters: bool,
    axis_to_btn_pressed: f32,
    axis_to_btn_released: f32,
}

impl GilrsBuilder {
    /// Create builder with default settings. Use `build()` to create `Gilrs`.
    pub fn new() -> Self {
        GilrsBuilder {
            mappings: MappingDb::new(),
            default_filters: true,
            axis_to_btn_pressed: 0.75,
            axis_to_btn_released: 0.65,
        }
    }

    /// If `true`, use [`axis_dpad_to_button`](ev/filter/fn.axis_dpad_to_button.html),
    /// [`Jitter`](ev/filter/struct.Jitter.html) and [`deadzone`](ev/filter/fn.deadzone.html)
    /// filters with default parameters. Defaults to `true`.
    pub fn with_default_filters(mut self, default_filters: bool) -> Self {
        self.default_filters = default_filters;

        self
    }

    /// Adds SDL mappings.
    pub fn add_mappings(mut self, mappings: &str) -> Self {
        self.mappings.insert(mappings);

        self
    }

    /// Adds SDL mappings from env.
    pub fn add_env_mappings(mut self) -> Self {
        self.mappings.add_env_mappings();

        self
    }

    /// Adds SDL mappings included from https://github.com/gabomdq/SDL_GameControllerDB
    pub fn add_included_mappings(mut self) -> Self {
        self.mappings.add_included_mappings();

        self
    }

    /// Sets values on which `ButtonPressed` and `ButtonReleased` events will be emitted. Panics if
    ///  `pressed ≤ released` or if one of values is outside [0.0, 1.0].
    ///
    /// Defaults to 0.75 for `pressed` and 0.65 for `released`.
    pub fn set_axis_to_btn(mut self, pressed: f32, released: f32) -> Self {
        assert!(pressed > released);
        assert!(pressed >= 0.0 && pressed <= 1.0);
        assert!(released >= 0.0 && released <= 1.0);

        self.axis_to_btn_pressed = pressed;
        self.axis_to_btn_released = released;

        self
    }

    /// Creates `Gilrs`.
    pub fn build(self) -> Gilrs {
        let mut gilrs = Gilrs {
            inner: platform::Gilrs::new(),
            next_id: 0,
            tx: server::init(),
            counter: 0,
            mappings: self.mappings,
            default_filters: self.default_filters,
            events: VecDeque::new(),
            axis_to_btn_pressed: self.axis_to_btn_pressed,
            axis_to_btn_released: self.axis_to_btn_released,
        };
        gilrs.finish_gamepads_creation();
        gilrs.create_ff_devices();

        gilrs
    }
}

/// Iterator over all connected gamepads.
pub struct ConnectedGamepadsIterator<'a>(&'a Gilrs, usize);

impl<'a> Iterator for ConnectedGamepadsIterator<'a> {
    type Item = (usize, &'a Gamepad);

    fn next(&mut self) -> Option<(usize, &'a Gamepad)> {
        loop {
            if self.1 == self.0.inner.last_gamepad_hint() {
                return None;
            }

            if let Some(gp) = self.0.connected_gamepad(self.1) {
                let idx = self.1;
                self.1 += 1;
                return Some((idx, gp));
            }

            self.1 += 1;
        }
    }
}

/// Iterator over all connected gamepads.
pub struct ConnectedGamepadsMutIterator<'a>(&'a mut Gilrs, usize);

impl<'a> Iterator for ConnectedGamepadsMutIterator<'a> {
    type Item = (usize, &'a mut Gamepad);

    fn next(&mut self) -> Option<(usize, &'a mut Gamepad)> {
        loop {
            if self.1 == self.0.inner.last_gamepad_hint() {
                return None;
            }

            if let Some(gp) = self.0.connected_gamepad_mut(self.1) {
                let idx = self.1;
                self.1 += 1;
                let gp = unsafe { &mut *(gp as *mut _) };
                return Some((idx, gp));
            }

            self.1 += 1;
        }
    }
}

/// Represents game controller.
///
/// Using this struct you can access cached gamepad state, informations about gamepad such as name
/// or UUID and manage force feedback effects.
#[derive(Debug)]
pub struct Gamepad {
    inner: platform::Gamepad,
    state: GamepadState,
    status: Status,
    mapping: Mapping,
    tx: Sender<Message>,
    id: usize,
}

impl Gamepad {
    fn new(gamepad: platform::Gamepad, status: Status) -> Self {
        Gamepad {
            inner: gamepad,
            state: GamepadState::new(),
            status,
            mapping: Mapping::new(),
            tx: ::std::sync::mpsc::channel().0,
            id: usize::max_value(),
        }
    }

    /// Returns the mapping name if it exists otherwise returns the os provided name.
    /// Warning: May change from os provided name to mapping name after the first call of event_next.
    pub fn name(&self) -> &str {
        let map_name = self.map_name();
        if map_name.is_empty() {
            self.os_name()
        } else {
            map_name
        }
    }

    /// Returns the name of the mapping used by the gamepad.
    /// Warning: Is an empty string until the first call of event_next.
    pub fn map_name(&self) -> &str {
        &self.mapping.name()
    }

    /// Returns the name of the gamepad supplied by the OS.
    pub fn os_name(&self) -> &str {
        self.inner.name()
    }

    /// Returns gamepad's UUID.
    pub fn uuid(&self) -> Uuid {
        self.inner.uuid()
    }

    /// Returns cached gamepad state.
    pub fn state(&self) -> &GamepadState {
        &self.state
    }

    /// Returns current gamepad's status, which can be `Connected`, `Disconnected` or `NotObserved`.
    /// Only connected gamepads generate events. Disconnected gamepads retain their name and UUID.
    /// Cached state of disconnected and not observed gamepads is 0 (false for buttons and 0.0 for
    /// axis) and all actions preformed on such gamepad are no-op.
    pub fn status(&self) -> Status {
        self.status
    }

    /// Returns true if gamepad is connected.
    pub fn is_connected(&self) -> bool {
        self.status == Status::Connected
    }

    /// Examines cached gamepad state to check if given button is pressed. Panics if `btn` is
    /// `Unknown`.
    ///
    /// If you know `Code` of the element that you want to examine, it's recommended to use methods
    /// directly on `State`, because this version have to check which `Code` is mapped to element of
    /// gamepad.
    pub fn is_pressed(&self, btn: Button) -> bool {
        assert_ne!(btn, Button::Unknown);

        self.button_code(btn)
            .or_else(|| btn.to_nec())
            .map(|nec| self.state.is_pressed(&nec))
            .unwrap_or(false)
    }

    /// Examines cached gamepad state to check axis's value. Panics if `axis` is `Unknown`.
    ///
    /// If you know `Code` of the element that you want to examine, it's recommended to use methods
    /// directly on `State`, because this version have to check which `Code` is mapped to element of
    /// gamepad.
    pub fn value(&self, axis: Axis) -> f32 {
        assert_ne!(axis, Axis::Unknown);

        self.axis_code(axis)
            .map(|nec| self.state.value(&nec))
            .unwrap_or(0.0)
    }

    /// Returns button state and when it changed.
    ///
    /// If you know `Code` of the element that you want to examine, it's recommended to use methods
    /// directly on `State`, because this version have to check which `Code` is mapped to element of
    /// gamepad.
    pub fn button_data(&self, btn: Button) -> Option<&ButtonData> {
        self.button_code(btn)
            .and_then(|nec| self.state.button_data(&nec))
    }

    /// Returns axis state and when it changed.
    ///
    /// If you know `Code` of the element that you want to examine, it's recommended to use methods
    /// directly on `State`, because this version have to check which `Code` is mapped to element of
    /// gamepad.
    pub fn axis_data(&self, axis: Axis) -> Option<&AxisData> {
        self.axis_code(axis)
            .and_then(|nec| self.state.axis_data(&nec))
    }

    /// Returns device's power supply state. See [`PowerInfo`](enum.PowerInfo.html) for details.
    pub fn power_info(&self) -> PowerInfo {
        self.inner.power_info()
    }

    /// Returns source of gamepad mapping. Can be used to filter gamepads which do not provide
    /// unified controller layout.
    ///
    /// ```
    /// use gilrs::MappingSource;
    /// # let mut gilrs = gilrs::Gilrs::new();
    ///
    /// for (_, gamepad) in gilrs.gamepads().filter(
    ///     |gp| gp.1.mapping_source() != MappingSource::None)
    /// {
    ///     println!("{} is ready to use!", gamepad.name());
    /// }
    /// ```
    pub fn mapping_source(&self) -> MappingSource {
        if self.mapping.is_default() {
            // TODO: check if it's Driver or None
            MappingSource::Driver
        } else {
            MappingSource::SdlMappings
        }
    }

    /// Sets gamepad's mapping and returns SDL2 representation of them. Returned mappings may not be
    /// compatible with SDL2 - if it is important, use
    /// [`set_mapping_strict()`](#method.set_mapping_strict).
    ///
    /// The `name` argument can be a string slice with custom gamepad name or `None`. If `None`,
    /// gamepad name reported by driver will be used.
    ///
    /// # Errors
    ///
    /// This function return error if `name` contains comma, `mapping` have axis and button entry
    /// for same element (for example `Axis::LetfTrigger` and `Button::LeftTrigger`) or gamepad does
    /// not have any element with `EvCode` used in mapping. `Button::Unknown` and
    /// `Axis::Unknown` are not allowd as keys to `mapping` – in this case,
    /// `MappingError::UnknownElement` is returned.
    ///
    /// Error is also returned if this function is not implemented or gamepad is not connected.
    ///
    /// # Example
    ///
    /// ```
    /// use gilrs::{Mapping, Button};
    ///
    /// # let mut gilrs = gilrs::Gilrs::new();
    /// let mut data = Mapping::new();
    /// // …
    ///
    /// // or `match gilrs[0].set_mapping(&data, None) {`
    /// match gilrs[0].set_mapping(&data, "Custom name") {
    ///     Ok(sdl) => println!("SDL2 mapping: {}", sdl),
    ///     Err(e) => println!("Failed to set mapping: {}", e),
    /// };
    /// ```
    ///
    /// See also `examples/mapping.rs`.
    pub fn set_mapping<'a, O: Into<Option<&'a str>>>(
        &mut self,
        mapping: &MappingData,
        name: O,
    ) -> Result<String, MappingError> {
        if !self.is_connected() {
            return Err(MappingError::NotConnected);
        }

        let name = match name.into() {
            Some(s) => s,
            None => self.inner.name(),
        };

        let (mapping, s) = Mapping::from_data(
            mapping,
            self.inner.buttons(),
            self.inner.axes(),
            name,
            self.uuid(),
        )?;
        self.mapping = mapping;

        Ok(s)
    }

    /// Similar to [`set_mapping()`](#method.set_mapping) but returned string should be compatible
    /// with SDL2.
    ///
    /// # Errors
    ///
    /// Returns `MappingError::NotSdl2Compatible` if `mapping` have an entry for `Button::{C, Z}`
    /// or `Axis::{LeftZ, RightZ}`.
    pub fn set_mapping_strict<'a, O: Into<Option<&'a str>>>(
        &mut self,
        mapping: &MappingData,
        name: O,
    ) -> Result<String, MappingError> {
        if mapping.button(Button::C).is_some() || mapping.button(Button::Z).is_some()
            || mapping.axis(Axis::LeftZ).is_some()
            || mapping.axis(Axis::RightZ).is_some()
        {
            Err(MappingError::NotSdl2Compatible)
        } else {
            self.set_mapping(mapping, name)
        }
    }

    /// Returns true if force feedback is supported by device.
    pub fn is_ff_supported(&self) -> bool {
        self.inner.is_ff_supported()
    }

    /// Change gamepad position used by force feedback effects.
    pub fn set_listener_position<Vec3: Into<[f32; 3]>>(
        &self,
        position: Vec3,
    ) -> Result<(), FfError> {
        if !self.is_connected() {
            Err(FfError::Disconnected(self.id))
        } else if !self.is_ff_supported() {
            Err(FfError::FfNotSupported(self.id))
        } else {
            self.tx.send(Message::SetListenerPosition {
                id: self.id,
                position: position.into(),
            })?;
            Ok(())
        }
    }

    /// Returns `AxisOrBtn` mapped to `Code`.
    pub fn axis_or_btn_name(&self, ec: Code) -> Option<AxisOrBtn> {
        self.mapping.map(&ec.0)
    }

    /// Returns `Code` associated with `btn`.
    pub fn button_code(&self, btn: Button) -> Option<Code> {
        self.mapping
            .map_rev(&AxisOrBtn::Btn(btn))
            .map(|nec| Code(nec))
    }

    /// Returns `Code` associated with `axis`.
    pub fn axis_code(&self, axis: Axis) -> Option<Code> {
        self.mapping
            .map_rev(&AxisOrBtn::Axis(axis))
            .map(|nec| Code(nec))
    }

    /// Returns area in which axis events should be ignored.
    pub fn deadzone(&self, axis: Code) -> Option<f32> {
        self.inner.axis_info(axis.0).map(|i| i.deadzone())
    }

    /// Returns ID of gamepad.
    pub fn id(&self) -> usize {
        self.id
    }
}

// TODO: use pub(crate)
impl AsInner<platform::Gamepad> for Gamepad {
    fn as_inner(&self) -> &platform::Gamepad {
        &self.inner
    }

    fn as_inner_mut(&mut self) -> &mut platform::Gamepad {
        &mut self.inner
    }
}

// TODO: use pub(crate)
pub trait GamepadImplExt {
    fn from_inner_status(inner: platform::Gamepad, status: Status) -> Self;
}

// TODO: use pub(crate)
impl GamepadImplExt for Gamepad {
    fn from_inner_status(inner: platform::Gamepad, status: Status) -> Self {
        Self::new(inner, status)
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
/// Status of gamepad's connection.
///
/// Only connected gamepads generate events. Disconnected gamepads retain their name and UUID.
/// Cached state of disconnected and not observed gamepads is 0 (false for buttons and 0.0 for
/// axis) and all actions preformed on such gamepad are no-op.
pub enum Status {
    Connected,
    Disconnected,
    NotObserved,
}

/// State of device's power supply.
///
/// Battery level is reported as integer between 0 and 100.
///
/// ## Example
///
/// ```
/// use gilrs::PowerInfo;
/// # let gilrs = gilrs::Gilrs::new();
///
/// match gilrs.gamepad(0).power_info() {
///     PowerInfo::Discharging(lvl) if lvl <= 10 => println!("Low battery level, you should \
///                                                           plug your gamepad"),
///     _ => (),
/// };
/// ```
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum PowerInfo {
    /// Failed to determine power status.
    Unknown,
    /// Device doesn't have battery.
    Wired,
    /// Device is running on the battery.
    Discharging(u8),
    /// Battery is charging.
    Charging(u8),
    /// Battery is charged.
    Charged,
}

/// Source of gamepad mappings.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum MappingSource {
    /// Gamepad uses SDL mappings.
    SdlMappings,
    /// Gamepad does not use any mappings but driver should provide unified controller layout.
    Driver,
    /// Gamepad does not use any mappings and most gamepad events will probably be `Button::Unknown`
    /// or `Axis::Unknown`
    None,
}
