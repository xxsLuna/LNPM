use std::{
    net::IpAddr,
    sync::{
        Arc,
        atomic::{AtomicU16, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use surge_ping::{Client, Config, ICMP, PingIdentifier, PingSequence, SurgeError};
use tokio::net::lookup_host;

use crate::domain::{AddressFamily, PingSample, ProbeStatus, Target, unix_time_ms};

#[async_trait]
pub trait PingProbe: Send + Sync {
    async fn probe(&self, target: &Target) -> PingSample;
}

pub struct SurgePingProbe {
    client_v4: Client,
    client_v6: Client,
    identifier: AtomicU16,
}

impl SurgePingProbe {
    pub async fn new() -> Result<Arc<Self>, String> {
        let client_v4 = Client::new(&Config::default()).map_err(|error| error.to_string())?;
        let client_v6 = Client::new(&Config::builder().kind(ICMP::V6).build())
            .map_err(|error| error.to_string())?;
        Ok(Arc::new(Self {
            client_v4,
            client_v6,
            identifier: AtomicU16::new(1),
        }))
    }

    async fn resolve(&self, target: &Target) -> Result<IpAddr, String> {
        if let Ok(address) = target.host.parse::<IpAddr>() {
            if family_matches(address, target.address_family) {
                return Ok(address);
            }
            return Err("The IP address does not match the selected address family".into());
        }

        let addresses = lookup_host((target.host.as_str(), 0))
            .await
            .map_err(|error| error.to_string())?;
        addresses
            .map(|address| address.ip())
            .find(|address| family_matches(*address, target.address_family))
            .ok_or_else(|| "No matching IP address was returned by DNS".into())
    }
}

#[async_trait]
impl PingProbe for SurgePingProbe {
    async fn probe(&self, target: &Target) -> PingSample {
        let timestamp_ms = unix_time_ms();
        let address = match self.resolve(target).await {
            Ok(address) => address,
            Err(error) => {
                let mut sample =
                    PingSample::failure(target.id.clone(), timestamp_ms, ProbeStatus::DnsError);
                sample.error = Some(error);
                return sample;
            }
        };

        let client = if address.is_ipv4() {
            &self.client_v4
        } else {
            &self.client_v6
        };
        let identifier = self.identifier.fetch_add(1, Ordering::Relaxed);
        let mut pinger = client.pinger(address, PingIdentifier(identifier)).await;
        pinger.timeout(Duration::from_millis(target.timeout_ms));
        let payload = [0_u8; 32];

        match pinger.ping(PingSequence(0), &payload).await {
            Ok((_, duration)) => PingSample {
                target_id: target.id.clone(),
                timestamp_ms,
                latency_ms: Some(duration.as_secs_f64() * 1_000.0),
                status: ProbeStatus::Success,
                resolved_address: Some(address.to_string()),
                error: None,
            },
            Err(error) => {
                let status = map_error(&error);
                let mut sample = PingSample::failure(target.id.clone(), timestamp_ms, status);
                sample.resolved_address = Some(address.to_string());
                sample.error = Some(error.to_string());
                sample
            }
        }
    }
}

fn family_matches(address: IpAddr, family: AddressFamily) -> bool {
    match family {
        AddressFamily::Auto => true,
        AddressFamily::Ipv4 => address.is_ipv4(),
        AddressFamily::Ipv6 => address.is_ipv6(),
    }
}

fn map_error(error: &SurgeError) -> ProbeStatus {
    match error {
        SurgeError::Timeout { .. } => ProbeStatus::Timeout,
        SurgeError::NetworkError => ProbeStatus::Unreachable,
        SurgeError::IOError(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            ProbeStatus::PermissionDenied
        }
        SurgeError::IOError(_) => ProbeStatus::Unreachable,
        _ => ProbeStatus::Error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_addresses_by_family() {
        assert!(family_matches(
            "1.1.1.1".parse().unwrap(),
            AddressFamily::Auto
        ));
        assert!(family_matches(
            "1.1.1.1".parse().unwrap(),
            AddressFamily::Ipv4
        ));
        assert!(!family_matches(
            "1.1.1.1".parse().unwrap(),
            AddressFamily::Ipv6
        ));
        assert!(family_matches("::1".parse().unwrap(), AddressFamily::Ipv6));
    }

    #[tokio::test]
    async fn initializes_clients_inside_a_tokio_runtime() {
        let _ = SurgePingProbe::new().await;
    }
}
