//! # playit-operator
//!
//! A Kubernetes operator that reconciles [`crd::PlayitTunnel`] custom resources
//! into tunnels on [playit.gg](https://playit.gg). It is the playit equivalent
//! of the way a Cloudflare Tunnel ingress controller turns Kubernetes objects
//! into externally reachable endpoints — except playit is a layer-4 (TCP/UDP)
//! port allocator, so each `PlayitTunnel` maps a public address to one in-cluster
//! `Service` port.
//!
//! The actual playit.gg API access lives behind the [`provider::TunnelProvider`]
//! trait so the reconcile loop can be exercised without credentials (see
//! [`provider::DryRunProvider`]) and the real implementation can be filled in
//! independently (see [`provider::PlayitProvider`]).

pub mod controller;
pub mod crd;
pub mod error;
pub mod provider;
