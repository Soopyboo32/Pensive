//! Glides a scroll to a new offset with an ease-out, so a click or a wheel notch moves the page
//! instead of snapping it. The glide moves the page once each frame the display draws, by how
//! long it's been running. A new target mid-glide starts from where the glide was headed, so
//! notches in a row add up. A trackpad moves the page as it reports, and anything else that moves
//! the page ends the glide where the page is.

use std::{cell::Cell, rc::Rc, time::Duration};

use gpui::{
    App, Context, DispatchPhase, Div, EntityId, HitboxBehavior, ListState, Pixels, ScrollEvent,
    ScrollHandle, Stateful, UniformListScrollHandle, Window, canvas, point, prelude::*,
};
use settings::{AnySetting, Control, Page, Setting, Settings};
use theme::{Dp, dp};
use web_time::Instant;

/// How long a wheel notch takes to glide.
pub(crate) const WHEEL_GLIDE: Duration = Duration::from_millis(150);
/// How far a line of wheel scrolling goes, as in Chrome: a notch reports three lines, by the
/// system's default, and moves the page 100 pixels at 100% zoom.
pub(crate) const WHEEL_LINE: Dp = dp(100. / 3.);

pub(crate) static SMOOTH: Setting<bool> = Setting {
    key: "scrolling.smooth",
    name: "Smooth scrolling",
    description: "Each wheel notch glides the page instead of jumping it",
    default: true,
    control: Control::Toggle,
};

pub(crate) fn declare_settings(cx: &mut App) {
    let page = Page::new(&theme::APPEARANCE, "Scrolling");
    Settings::declare(page, [&SMOOTH as &dyn AnySetting], cx);
}

/// Something that scrolls up and down, whose offset runs from zero at the top down to minus the
/// most it can scroll.
pub(crate) trait Scrollable: Clone + 'static {
    fn top(&self) -> Pixels;
    /// How far the content can scroll, as a positive distance.
    fn most(&self) -> Pixels;
    fn set_top(&self, top: Pixels);
}

impl Scrollable for ScrollHandle {
    fn top(&self) -> Pixels {
        self.offset().y
    }

    fn most(&self) -> Pixels {
        self.max_offset().y
    }

    fn set_top(&self, top: Pixels) {
        self.set_offset(point(self.offset().x, top));
    }
}

/// A list measures its rows as it draws them, so how far it can scroll grows as it does.
impl Scrollable for ListState {
    fn top(&self) -> Pixels {
        self.scroll_px_offset_for_scrollbar().y
    }

    fn most(&self) -> Pixels {
        self.max_offset_for_scrollbar().y
    }

    fn set_top(&self, top: Pixels) {
        self.set_offset_from_scrollbar(point(Pixels::ZERO, top));
    }
}

#[derive(Clone, Copy)]
struct Glide {
    /// Tells this glide from one that replaced it, whose frames would otherwise step it too.
    id: u64,
    from: Pixels,
    /// Where the glide was asked to end. It's kept within the content each frame rather than
    /// once, since a list measures more rows as it scrolls and can then go further.
    to: Pixels,
    /// Where the glide last put the page, to tell when something else has moved it.
    at: Pixels,
    start: Instant,
    duration: Duration,
}

struct Inner<T> {
    target: T,
    glide: Cell<Option<Glide>>,
    /// The last glide's id.
    glides: Cell<u64>,
}

/// Moves `T` smoothly. Clones share the scroll, so a listener can hold one.
#[derive(Clone)]
pub(crate) struct SmoothScroll<T: Scrollable = ScrollHandle>(Rc<Inner<T>>);

impl SmoothScroll {
    pub fn new() -> Self {
        Self::of(ScrollHandle::new())
    }

    /// Glides a uniform list, which scrolls through the handle underneath its own.
    pub fn uniform_list(handle: &UniformListScrollHandle) -> Self {
        Self::of(handle.0.borrow().base_handle.clone())
    }

    /// The handle to track and read. Moving the page through it ends the glide.
    pub fn handle(&self) -> &ScrollHandle {
        &self.0.target
    }

    /// Scrolls `element`'s children through this, gliding each wheel notch. The element is
    /// hidden rather than scrolled, so the wheel is this one's alone.
    pub fn scrolls<V: 'static>(&self, element: Stateful<Div>, cx: &Context<V>) -> Stateful<Div> {
        element
            .overflow_hidden()
            .track_scroll(self.handle())
            .on_scroll(self.wheel_listener(cx))
    }
}

impl<T: Scrollable> SmoothScroll<T> {
    pub fn of(target: T) -> Self {
        Self(Rc::new(Inner {
            target,
            glide: Cell::new(None),
            glides: Cell::new(0),
        }))
    }

    /// Whether a glide is still moving the page.
    pub fn is_gliding(&self) -> bool {
        self.glide().is_some()
    }

    /// Glides to vertical offset `to`, kept within the content, over `duration`.
    pub fn glide_to<V: 'static>(
        &self,
        to: Pixels,
        duration: Duration,
        window: &Window,
        cx: &Context<V>,
    ) {
        self.start(to, duration, cx.entity_id(), window, cx);
    }

    /// Moves straight to vertical offset `to`, kept within the content, ending any glide.
    pub fn jump_to(&self, to: Pixels) {
        self.stop();
        self.0.target.set_top(self.clamp(to));
    }

    /// Ends any glide where the page is, as before moving the page some other way.
    pub fn stop(&self) {
        self.0.glide.set(None);
    }

    /// Scrolls for a wheel event over the page, and redraws `view`, which draws it. A wheel
    /// notch glides, unless the settings turn that off, and a trackpad moves the page at once.
    pub fn wheel(&self, event: &ScrollEvent, view: EntityId, window: &Window, cx: &mut App) {
        let line = WHEEL_LINE.to_pixels(window.rem_size());
        let delta = event.delta.pixel_delta(line).y;
        if delta == Pixels::ZERO {
            return;
        }
        if event.delta.precise() || !SMOOTH.get(cx) {
            self.jump_to(self.0.target.top() + delta);
        } else {
            self.start(self.headed() + delta, WHEEL_GLIDE, view, window, cx);
        }
        cx.notify(view);
    }

    /// Scrolls for each wheel event it's handed, and stops the event there.
    pub fn wheel_listener<V: 'static>(
        &self,
        cx: &Context<V>,
    ) -> impl Fn(&ScrollEvent, &mut Window, &mut App) + 'static {
        let (scroll, view) = (self.clone(), cx.entity_id());
        move |event, window, cx| {
            scroll.wheel(event, view, window, cx);
            cx.stop_propagation();
        }
    }

    /// A layer that takes the wheel before the element under it, for an element that scrolls
    /// itself, such as a list. Put it after that element, in a parent it fills.
    pub fn wheel_layer<V: 'static>(&self, cx: &Context<V>) -> impl IntoElement + use<T, V> {
        self.wheel_layer_then(cx, |_, _, _| {})
    }

    /// [`SmoothScroll::wheel_layer`] that hands each wheel event over it to `then` as well, as
    /// the element would have.
    pub fn wheel_layer_then<V, F>(
        &self,
        cx: &Context<V>,
        then: F,
    ) -> impl IntoElement + use<T, V, F>
    where
        V: 'static,
        F: Fn(&ScrollEvent, &mut Window, &mut App) + 'static,
    {
        let listen = self.wheel_listener(cx);
        canvas(
            |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
            move |_, hitbox, window, _| {
                // Bubbling reaches the last listener first, so this one runs before the
                // element's, and stops it.
                window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble && hitbox.should_handle_scroll(window) {
                        then(event, window, cx);
                        listen(event, window, cx);
                    }
                });
            },
        )
        .absolute()
        .inset_0()
    }

    /// The running glide, which ends if something else has moved the page since it last did.
    fn glide(&self) -> Option<Glide> {
        let glide = self
            .0
            .glide
            .get()
            .filter(|glide| !moved(self.0.target.top(), glide.at));
        self.0.glide.set(glide);
        glide
    }

    /// Where the page is headed: the running glide's end, or where it is. A list may go further
    /// than it has measured, so the end is only kept within the content once the page has gone
    /// as far as it can, where a notch back should turn at once.
    fn headed(&self) -> Pixels {
        match self.glide() {
            Some(glide) if self.0.target.top() <= -self.0.target.most() => self.clamp(glide.to),
            Some(glide) => glide.to,
            None => self.0.target.top(),
        }
    }

    fn start(&self, to: Pixels, duration: Duration, view: EntityId, window: &Window, cx: &App) {
        let from = self.0.target.top();
        let id = self.0.glides.get() + 1;
        self.0.glides.set(id);
        self.0.glide.set(Some(Glide {
            id,
            from,
            to,
            at: from,
            start: cx.background_executor().now(),
            duration,
        }));
        self.next_frame(id, view, window);
    }

    /// Steps glide `id` just before the display draws the next frame, which shows the page
    /// there, until it lands.
    fn next_frame(&self, id: u64, view: EntityId, window: &Window) {
        let scroll = self.clone();
        window.on_next_frame(move |window, cx| {
            let Some(done) = scroll.step(id, cx.background_executor().now()) else {
                return;
            };
            cx.notify(view);
            if done < 1. {
                scroll.next_frame(id, view, window);
            }
        });
    }

    /// Moves the page as far along glide `id` as `now` puts it, easing out, and returns how far
    /// that is, unless another glide or a jump replaced it or something else moved the page.
    fn step(&self, id: u64, now: Instant) -> Option<f32> {
        let glide = self.glide().filter(|glide| glide.id == id)?;
        let done = (now - glide.start).div_duration_f32(glide.duration).min(1.);
        let eased = 1. - (1. - done).powi(3);
        let to = self.clamp(glide.to);
        self.0
            .target
            .set_top(glide.from + (to - glide.from) * eased);
        // Read back what the scroll took, which a list keeps within what it can scroll.
        let at = self.0.target.top();
        self.0
            .glide
            .set((done < 1.).then_some(Glide { at, ..glide }));
        Some(done)
    }

    fn clamp(&self, offset: Pixels) -> Pixels {
        offset.clamp(-self.0.target.most(), Pixels::ZERO)
    }
}

/// Whether the page is far enough from where a glide put it that something else moved it.
fn moved(top: Pixels, at: Pixels) -> bool {
    f32::from(top - at).abs() > 0.5
}

#[cfg(test)]
#[expect(clippy::disallowed_methods, reason = "tests scroll by window pixels")]
mod tests {
    use gpui::{
        Entity, ListAlignment, Modifiers, ScrollDelta, TestAppContext, VisualTestContext, div,
        list, px,
    };

    use super::*;
    use crate::test_support::run_frames;

    /// A list of a hundred rows, each 20 pixels tall, that glides for the wheel.
    struct Rows {
        list: ListState,
        scroll: SmoothScroll<ListState>,
    }

    impl Render for Rows {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .relative()
                .size_full()
                .child(
                    list(self.list.clone(), |_, _, _| {
                        div().h(px(20.)).into_any_element()
                    })
                    .size_full(),
                )
                .child(self.scroll.wheel_layer(cx))
        }
    }

    fn rows(cx: &mut TestAppContext) -> (Entity<Rows>, &mut VisualTestContext) {
        crate::test_support::init(cx);
        let (rows, cx) = cx.add_window_view(|_, _| {
            let list = ListState::new(100, ListAlignment::Top, px(0.)).measure_all();
            Rows {
                scroll: SmoothScroll::of(list.clone()),
                list,
            }
        });
        cx.run_until_parked();
        (rows, cx)
    }

    fn wheel(delta: ScrollDelta, cx: &mut VisualTestContext) {
        let position = point(px(10.), px(10.));
        cx.simulate_mouse_move(position, None, Modifiers::none());
        cx.simulate_event(ScrollEvent {
            position,
            delta,
            ..Default::default()
        });
    }

    fn top(rows: &Entity<Rows>, cx: &mut VisualTestContext) -> Pixels {
        rows.read_with(cx, |rows, _| rows.list.top())
    }

    #[gpui::test]
    fn a_wheel_notch_glides_a_list_and_notches_add_up(cx: &mut TestAppContext) {
        let (rows, cx) = rows(cx);
        wheel(ScrollDelta::Pixels(point(px(0.), px(-1.))), cx);
        assert_eq!(top(&rows, cx), px(-1.), "a trackpad moves the list at once");

        wheel(ScrollDelta::Lines(point(0., -2.)), cx);
        assert_eq!(
            top(&rows, cx),
            px(-1.),
            "a notch waits a step before moving"
        );
        run_frames(cx, WHEEL_GLIDE / 3);
        let partway = top(&rows, cx);
        wheel(ScrollDelta::Lines(point(0., -2.)), cx);
        run_frames(cx, WHEEL_GLIDE * 2);
        let done = top(&rows, cx);
        let notch = ScrollDelta::Lines(point(0., -2.))
            .pixel_delta(cx.update(|window, _| WHEEL_LINE.to_pixels(window.rem_size())));
        assert!(
            px(-1.) > partway && partway > done,
            "{partway:?} then {done:?}"
        );
        assert!(
            (done - (px(-1.) + notch.y * 2.)).abs() < px(0.5),
            "two notches land twice as far: {done:?}"
        );
    }

    #[gpui::test]
    fn notches_past_the_rows_measured_so_far_go_the_whole_way(cx: &mut TestAppContext) {
        crate::test_support::init(cx);
        // Without measuring every row, the list only knows the rows it has drawn, which reach
        // 200 pixels past the window, as the editor's do.
        let (rows, cx) = cx.add_window_view(|_, _| {
            let list = ListState::new(1_000, ListAlignment::Top, px(200.));
            Rows {
                scroll: SmoothScroll::of(list.clone()),
                list,
            }
        });
        cx.run_until_parked();
        for _ in 0..4 {
            wheel(ScrollDelta::Lines(point(0., -3.)), cx);
        }
        run_frames(cx, WHEEL_GLIDE * 2);
        let notch = cx.update(|window, _| WHEEL_LINE.to_pixels(window.rem_size())) * 3.;
        let top = top(&rows, cx);
        assert!((top + notch * 4.).abs() < px(0.5), "{top:?}");
    }

    #[gpui::test]
    fn moving_the_list_some_other_way_ends_the_glide(cx: &mut TestAppContext) {
        let (rows, cx) = rows(cx);
        wheel(ScrollDelta::Lines(point(0., -5.)), cx);
        run_frames(cx, WHEEL_GLIDE / 3);
        rows.update(cx, |rows, _| rows.list.set_top(px(-7.)));
        run_frames(cx, WHEEL_GLIDE * 2);
        assert_eq!(top(&rows, cx), px(-7.));
        assert!(rows.read_with(cx, |rows, _| !rows.scroll.is_gliding()));
    }

    #[gpui::test]
    fn without_smooth_scrolling_a_notch_moves_at_once(cx: &mut TestAppContext) {
        let (rows, cx) = rows(cx);
        cx.update(|_, cx| SMOOTH.set(false, cx));
        wheel(ScrollDelta::Lines(point(0., -2.)), cx);
        assert!(top(&rows, cx) < px(0.));
        assert!(rows.read_with(cx, |rows, _| !rows.scroll.is_gliding()));
    }

    #[gpui::test]
    fn a_glide_stops_at_the_end(cx: &mut TestAppContext) {
        let (rows, cx) = rows(cx);
        wheel(ScrollDelta::Lines(point(0., -10_000.)), cx);
        run_frames(cx, WHEEL_GLIDE * 2);
        let most = rows.read_with(cx, |rows, _| rows.list.most());
        assert!(most > px(0.));
        assert_eq!(top(&rows, cx), -most);
    }
}
