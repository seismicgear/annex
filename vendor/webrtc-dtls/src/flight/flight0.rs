use std::fmt;
use std::sync::atomic::Ordering;

use async_trait::async_trait;
use rand::Rng;

use super::flight2::*;
use super::*;
use crate::config::*;
use crate::conn::*;
use crate::error::Error;
use crate::extension::*;
use crate::handshake::*;
use crate::record_layer::record_layer_header::*;
use crate::*;

#[derive(Debug, PartialEq)]
pub(crate) struct Flight0;

impl fmt::Display for Flight0 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Flight 0")
    }
}

#[async_trait]
impl Flight for Flight0 {
    async fn parse(
        &self,
        _tx: &mut mpsc::Sender<mpsc::Sender<()>>,
        state: &mut State,
        cache: &HandshakeCache,
        cfg: &HandshakeConfig,
    ) -> Result<Box<dyn Flight + Send + Sync>, (Option<Alert>, Option<Error>)> {
        let (seq, msgs) = match cache
            .full_pull_map(
                0,
                &[HandshakeCachePullRule {
                    typ: HandshakeType::ClientHello,
                    epoch: cfg.initial_epoch,
                    is_client: true,
                    optional: false,
                }],
            )
            .await
        {
            Ok((seq, msgs)) => (seq, msgs),
            Err(_) => return Err((None, None)),
        };

        state.handshake_recv_sequence = seq;

        if let Some(message) = msgs.get(&HandshakeType::ClientHello) {
            // Validate type
            let client_hello = match message {
                HandshakeMessage::ClientHello(client_hello) => client_hello,
                _ => {
                    return Err((
                        Some(Alert {
                            alert_level: AlertLevel::Fatal,
                            alert_description: AlertDescription::InternalError,
                        }),
                        None,
                    ))
                }
            };

            if client_hello.version != PROTOCOL_VERSION1_2 {
                return Err((
                    Some(Alert {
                        alert_level: AlertLevel::Fatal,
                        alert_description: AlertDescription::ProtocolVersion,
                    }),
                    Some(Error::ErrUnsupportedProtocolVersion),
                ));
            }

            state.remote_random = client_hello.random.clone();

            if let Ok(id) =
                find_matching_cipher_suite(&client_hello.cipher_suites, &cfg.local_cipher_suites)
            {
                if let Ok(cipher_suite) = cipher_suite_for_id(id) {
                    log::debug!(
                        "[handshake:{}] use cipher suite: {}",
                        srv_cli_str(state.is_client),
                        cipher_suite.to_string()
                    );
                    let mut cs = state.cipher_suite.lock().await;
                    *cs = Some(cipher_suite);
                }
            } else {
                return Err((
                    Some(Alert {
                        alert_level: AlertLevel::Fatal,
                        alert_description: AlertDescription::InsufficientSecurity,
                    }),
                    Some(Error::ErrCipherSuiteNoIntersection),
                ));
            }

            for extension in &client_hello.extensions {
                match extension {
                    Extension::SupportedEllipticCurves(e) => {
                        // Ignore unknown/GREASE groups while preserving the peer's
                        // preference among curves implemented by this version.
                        let supported = e.elliptic_curves.iter().copied().find(|curve| {
                            matches!(
                                curve,
                                crate::curve::named_curve::NamedCurve::X25519
                                    | crate::curve::named_curve::NamedCurve::P256
                                    | crate::curve::named_curve::NamedCurve::P384
                            )
                        });
                        let Some(curve) = supported else {
                            return Err((
                                Some(Alert {
                                    alert_level: AlertLevel::Fatal,
                                    alert_description: AlertDescription::InsufficientSecurity,
                                }),
                                Some(Error::ErrNoSupportedEllipticCurves),
                            ));
                        };
                        state.named_curve = curve;
                    }
                    Extension::UseSrtp(e) => {
                        if let Ok(profile) = find_matching_srtp_profile(
                            &e.protection_profiles,
                            &cfg.local_srtp_protection_profiles,
                        ) {
                            state.srtp_protection_profile = profile;
                        } else {
                            return Err((
                                Some(Alert {
                                    alert_level: AlertLevel::Fatal,
                                    alert_description: AlertDescription::InsufficientSecurity,
                                }),
                                Some(Error::ErrServerNoMatchingSrtpProfile),
                            ));
                        }
                    }
                    Extension::UseExtendedMasterSecret(_) => {
                        if cfg.extended_master_secret != ExtendedMasterSecretType::Disable {
                            state.extended_master_secret = true;
                        }
                    }
                    Extension::ServerName(e) => {
                        state.server_name = e.server_name.clone(); // remote server name
                    }
                    _ => {}
                }
            }

            if cfg.extended_master_secret == ExtendedMasterSecretType::Require
                && !state.extended_master_secret
            {
                return Err((
                    Some(Alert {
                        alert_level: AlertLevel::Fatal,
                        alert_description: AlertDescription::InsufficientSecurity,
                    }),
                    Some(Error::ErrServerRequiredButNoClientEms),
                ));
            }

            if state.local_keypair.is_none() {
                state.local_keypair = match state.named_curve.generate_keypair() {
                    Ok(local_keypar) => Some(local_keypar),
                    Err(err) => {
                        return Err((
                            Some(Alert {
                                alert_level: AlertLevel::Fatal,
                                alert_description: AlertDescription::IllegalParameter,
                            }),
                            Some(err),
                        ))
                    }
                };
            }

            Ok(Box::new(Flight2 {}))
        } else {
            Err((
                Some(Alert {
                    alert_level: AlertLevel::Fatal,
                    alert_description: AlertDescription::InternalError,
                }),
                None,
            ))
        }
    }

    async fn generate(
        &self,
        state: &mut State,
        _cache: &HandshakeCache,
        _cfg: &HandshakeConfig,
    ) -> Result<Vec<Packet>, (Option<Alert>, Option<Error>)> {
        // Initialize
        state.cookie = vec![0; COOKIE_LENGTH];
        rand::thread_rng().fill(state.cookie.as_mut_slice());

        //TODO: figure out difference between golang's atom store and rust atom store
        let zero_epoch = 0;
        state.local_epoch.store(zero_epoch, Ordering::SeqCst);
        state.remote_epoch.store(zero_epoch, Ordering::SeqCst);

        state.named_curve = DEFAULT_NAMED_CURVE;
        state.local_random.populate();

        Ok(vec![])
    }
}

#[cfg(test)]
mod supported_group_regression {
    use super::*;
    use crate::compression_methods::default_compression_methods;
    use crate::curve::named_curve::NamedCurve;
    use crate::extension::extension_supported_elliptic_curves::ExtensionSupportedEllipticCurves;
    use crate::handshake::handshake_message_client_hello::HandshakeMessageClientHello;
    use crate::handshake::handshake_random::HandshakeRandom;

    async fn parse_groups(
        groups: &[u16],
    ) -> (
        State,
        std::result::Result<Box<dyn Flight + Send + Sync>, (Option<Alert>, Option<Error>)>,
    ) {
        let cipher = CipherSuiteId::Tls_Ecdhe_Ecdsa_With_Aes_128_Gcm_Sha256;
        let message = HandshakeMessageClientHello {
            version: PROTOCOL_VERSION1_2,
            random: HandshakeRandom::default(),
            cookie: vec![],
            cipher_suites: vec![cipher],
            compression_methods: default_compression_methods(),
            extensions: vec![Extension::SupportedEllipticCurves(
                ExtensionSupportedEllipticCurves {
                    elliptic_curves: groups.iter().copied().map(NamedCurve::from).collect(),
                },
            )],
        };
        let mut bytes = vec![];
        Handshake::new(HandshakeMessage::ClientHello(message))
            .marshal(&mut bytes)
            .unwrap();
        // The groups are the final extension payload. Restore their original
        // wire values so GREASE exercises the real decoder, not a constructed enum.
        let start = bytes.len() - groups.len() * 2;
        for (index, group) in groups.iter().enumerate() {
            bytes[start + index * 2..start + index * 2 + 2].copy_from_slice(&group.to_be_bytes());
        }
        let mut cache = HandshakeCache::new();
        assert!(
            cache
                .push(bytes, 0, 0, HandshakeType::ClientHello, true)
                .await
        );
        let cfg = HandshakeConfig {
            local_cipher_suites: vec![cipher],
            ..Default::default()
        };
        let mut state = State::default();
        let (mut tx, _rx) = mpsc::channel(1);
        Flight0.generate(&mut state, &cache, &cfg).await.unwrap();
        let result = Flight0.parse(&mut tx, &mut state, &cache, &cfg).await;
        (state, result)
    }

    #[tokio::test]
    async fn unknown_wire_groups_before_supported_groups_reach_server_key_generation() {
        for (groups, expected) in [
            (vec![0x0a0a, 0x6399, 0x001d, 0x0017], NamedCurve::X25519),
            (vec![0x2a2a, 0x0017, 0x001d], NamedCurve::P256),
            (vec![0x3a3a, 0x0018, 0x001d], NamedCurve::P384),
            (vec![0x001d, 0x0017], NamedCurve::X25519),
        ] {
            let (state, result) = parse_groups(&groups).await;
            assert_eq!(result.unwrap().to_string(), "Flight 2");
            assert_eq!(state.named_curve, expected);
            assert_eq!(state.local_keypair.unwrap().curve, expected);
            assert!(state.cipher_suite.lock().await.is_some());
        }
    }

    #[tokio::test]
    async fn empty_and_unknown_only_groups_fail_without_generating_a_key() {
        for groups in [vec![], vec![0x0a0a, 0x6399, 0xffff]] {
            let (state, result) = parse_groups(&groups).await;
            let (alert, error) = result.unwrap_err();
            let alert = alert.unwrap();
            assert_eq!(alert.alert_level, AlertLevel::Fatal);
            assert_eq!(
                alert.alert_description,
                AlertDescription::InsufficientSecurity
            );
            assert!(matches!(error, Some(Error::ErrNoSupportedEllipticCurves)));
            assert!(state.local_keypair.is_none());
        }
    }
}
