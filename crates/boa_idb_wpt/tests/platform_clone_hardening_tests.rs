//! Regression tests for structured-clone platform-object identity.
//!
//! These run in the same `prepare` path as the WPT runner: a real
//! IndexedDB-backed Context with sealed platform shims.

use boa_engine::js_string;
use boa_idb_wpt::environment::{self, Backend};
use tempfile::tempdir;

fn eval_bool(script: &str) -> bool {
    let dir = tempdir().expect("temporary directory");
    let mut prepared =
        environment::prepare(Backend::Memory, dir.path()).expect("prepare WPT environment");
    let wrapped = format!("(function() {{\n{script}\n}})()");
    let value = prepared
        .context
        .eval(boa_engine::Source::from_bytes(&wrapped))
        .unwrap_or_else(|e| panic!("script failed: {e}\n{wrapped}"));
    value
        .as_boolean()
        .unwrap_or_else(|| panic!("expected boolean result from:\n{wrapped}"))
}

#[test]
fn forged_constructor_does_not_get_platform_clone() {
    assert!(eval_bool(
        r#"
        // Own `constructor` must be non-enumerable: an enumerable function
        // property fails clone for "function cannot be cloned", which is a
        // different path from platform-object recognition.
        const forged = { x: 1, y: 2, z: 3, w: 4 };
        Object.defineProperty(forged, 'constructor', {
            value: DOMPoint, enumerable: false, configurable: true, writable: true
        });
        const clone = structuredClone(forged);
        return Object.getPrototypeOf(clone) !== DOMPoint.prototype
            && Object.getPrototypeOf(clone) === Object.prototype
            && clone.x === 1;
        "#,
    ));
}

#[test]
fn forged_prototype_does_not_get_platform_clone() {
    assert!(eval_bool(
        r"
        const forged = Object.create(DOMPoint.prototype);
        forged.x = 9; forged.y = 8; forged.z = 7; forged.w = 6;
        const clone = structuredClone(forged);
        return Object.getPrototypeOf(clone) !== DOMPoint.prototype
            && Object.getPrototypeOf(clone) === Object.prototype;
        ",
    ));
}

#[test]
fn brand_is_not_exposed_on_global_or_shim_symbols() {
    assert!(eval_bool(
        r"
        if (globalThis.__boa_platform_clone_brand !== undefined) { return false; }
        if (globalThis.__boa_register_platform_clone !== undefined) { return false; }
        const globalSymbols = Object.getOwnPropertySymbols(globalThis);
        for (const sym of globalSymbols) {
            const fake = Object.create(DOMPoint.prototype);
            Object.defineProperty(fake, sym, {
                value: true, enumerable: false, configurable: true, writable: true
            });
            const clone = structuredClone(fake);
            if (Object.getPrototypeOf(clone) === DOMPoint.prototype) { return false; }
        }
        const shim = new DOMPoint(1, 2, 3, 4);
        const shimSymbols = Object.getOwnPropertySymbols(shim);
        for (const sym of shimSymbols) {
            const fake = Object.create(DOMPoint.prototype);
            Object.defineProperty(fake, sym, {
                value: true, enumerable: false, configurable: true, writable: true
            });
            const clone = structuredClone(fake);
            if (Object.getPrototypeOf(clone) === DOMPoint.prototype) { return false; }
        }
        return true;
        ",
    ));
}

#[test]
fn plain_object_with_event_or_messagechannel_name_serializes() {
    assert!(eval_bool(
        r#"
        function fakeNamed(name) {
            function Ctor() {}
            Object.defineProperty(Ctor, 'name', { value: name });
            const obj = { tagged: name };
            Object.setPrototypeOf(obj, Ctor.prototype);
            Object.defineProperty(obj, 'constructor', {
                value: Ctor, enumerable: false, configurable: true, writable: true
            });
            return obj;
        }
        const eventLike = fakeNamed('Event');
        const channelLike = fakeNamed('MessageChannel');
        // Non-callable postMessage: proves platform rejection is not triggered.
        // A real function property would fail for "function cannot be cloned".
        channelLike.postMessage = 1;
        const eventClone = structuredClone(eventLike);
        const channelClone = structuredClone(channelLike);
        let functionRejectIsNotPlatform = false;
        try {
            const o = { postMessage: function() {} };
            Object.defineProperty(o, 'constructor', {
                value: { name: 'MessageChannel' },
                enumerable: false
            });
            structuredClone(o);
        } catch (e) {
            functionRejectIsNotPlatform = String(e).indexOf('platform object') === -1;
        }
        return eventClone.tagged === 'Event'
            && channelClone.tagged === 'MessageChannel'
            && channelClone.postMessage === 1
            && functionRejectIsNotPlatform;
        "#,
    ));
}

#[test]
fn forged_messagechannel_prototype_does_not_reject() {
    assert!(eval_bool(
        r"
        const forged = Object.create(MessageChannel.prototype);
        forged.tag = 'plain';
        try {
            const clone = structuredClone(forged);
            return clone.tag === 'plain'
                && Object.getPrototypeOf(clone) === Object.prototype;
        } catch (e) {
            return false;
        }
        ",
    ));
}

#[test]
fn authentic_shims_keep_m5_clone_semantics() {
    assert!(eval_bool(
        r"
        const point = new DOMPoint(1, 2, 3, 4);
        const pointClone = structuredClone(point);
        if (Object.getPrototypeOf(pointClone) !== DOMPoint.prototype) { return false; }
        if (pointClone.x !== 1 || pointClone.y !== 2) { return false; }

        const blob = new Blob(['hi'], { type: 'text/plain' });
        const blobClone = structuredClone(blob);
        if (Object.getPrototypeOf(blobClone) !== Blob.prototype) { return false; }
        if (blobClone.type !== 'text/plain') { return false; }

        let eventRejected = false;
        try { structuredClone(new Event('x')); } catch (e) { eventRejected = true; }
        let channelRejected = false;
        try { structuredClone(new MessageChannel()); } catch (e) { channelRejected = true; }
        return eventRejected && channelRejected;
        ",
    ));
}

#[test]
fn registrar_is_absent_after_bootstrap() {
    let dir = tempdir().expect("temporary directory");
    let mut prepared =
        environment::prepare(Backend::Memory, dir.path()).expect("prepare WPT environment");
    let value = prepared
        .context
        .global_object()
        .get(
            js_string!("__boa_register_platform_clone"),
            &mut prepared.context,
        )
        .expect("property lookup");
    assert!(value.is_undefined());
}
