mod bench;

pub use bench::run_grpc;

// Generated protobuf/gRPC code lives here so the benchmark module can import it.
pub mod proto {
    pub mod echo {
        tonic::include_proto!("bench");
    }
    pub mod stream {
        tonic::include_proto!("stream");
    }
}