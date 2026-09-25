# TsinghuaKit engine

This crate contains the protocol implementation used by the public
[`tsinghua_kit`](../tsinghua-kit) SDK and the Flutter bridge compatibility
crate. Application integrations should depend on the SDK crate and must not
depend on this internal implementation crate directly.

The engine owns all HTTP dispatch, authentication, session state, response
parsing, and service caches. Its `ffi-bridge` feature is enabled only by the
Flutter compatibility package.
