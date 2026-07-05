pub mod client;
pub mod client_ops;
pub mod connection;
pub mod control_plane;
pub mod heartbeat;
pub mod message;
pub mod reconnect;
pub mod server;
pub mod supervisor;

pub use client::{connect_to_peer, ConnectOutcome};
pub use control_plane::{spawn_control_plane, ControlPlaneDeps, ControlPlaneObserver};
pub use reconnect::{spawn_reconnect, ReconnectDriver};
pub use supervisor::{spawn_connection_supervisor, ControlPlaneHost};
pub use client_ops::{
    build_stream_route, find_conn, find_conn_tx, notify_remote_control, stream_open_message,
    wait_for_stream_open_ack, ConnEndpoints, ConnectionMap,
};
pub use connection::{
    spawn_peer_connection, PeerConnectionHandle, PeerEvent, REMOTE_PEER_HEARTBEAT_TIMEOUT,
};
pub use heartbeat::build_heartbeat;
pub use message::{
    Capabilities, Codec, CodecParams, DeviceDescriptor, Endpoint, HeartbeatStreamStats,
    SignalingMessage, SourceKind, StreamAction, PROTOCOL_VERSION,
};
pub use server::{accept_pending_as, PendingPeer, SignalingServer, SignalingServerHandle};
