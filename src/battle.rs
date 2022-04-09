use crate::datachannel;
use crate::input;
use crate::mgba;
use crate::protocol;
use crate::signor;
use prost::Message;
use rand::Rng;
use rand::SeedableRng;
use sha3::{digest::ExtendableOutput, Shake128};
use std::io::Read;
use std::io::Write;
use std::ops::Neg;
use subtle::ConstantTimeEq;

pub struct Init {
    input_delay: u32,
    marshaled: [u8; 0x100],
}

struct BattleHolder {
    number: u32,
    battle: Option<Battle>,
}

enum Negotiation {
    NotReady,
    Negotiated {
        dc: std::sync::Arc<datachannel::DataChannel>,
        rng: rand_pcg::Mcg128Xsl64,
    },
    Err(anyhow::Error),
}

pub struct Match {
    negotiation: parking_lot::Mutex<Negotiation>,
    session_id: String,
    match_type: u16,
    game_title: String,
    game_crc32: u32,
    won_last_battle: bool,
    battle_holder: parking_lot::Mutex<BattleHolder>,
    aborted: std::sync::atomic::AtomicBool,
}

fn make_rng_commitment(nonce: &[u8]) -> anyhow::Result<[u8; 32]> {
    let mut shake128 = sha3::Shake128::default();
    shake128.write_all(b"syncrand:nonce:")?;
    shake128.write_all(&nonce)?;

    let mut commitment = [0u8; 32];
    shake128
        .finalize_xof()
        .read_exact(commitment.as_mut_slice())?;

    Ok(commitment)
}

impl Match {
    pub fn new(session_id: String, match_type: u16, game_title: String, game_crc32: u32) -> Self {
        Match {
            negotiation: parking_lot::Mutex::new(Negotiation::NotReady),
            session_id,
            match_type,
            game_title,
            game_crc32,
            won_last_battle: false,
            battle_holder: parking_lot::Mutex::new(BattleHolder {
                number: 0,
                battle: None,
            }),
            aborted: false.into(),
        }
    }

    pub fn abort(&mut self) {
        self.aborted
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn aborted(&mut self) -> bool {
        self.aborted.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn lock_battle(&self) -> parking_lot::MappedMutexGuard<Option<Battle>> {
        parking_lot::MutexGuard::map(self.battle_holder.lock(), |battle_holder| {
            &mut battle_holder.battle
        })
    }

    #[tokio::main(flavor = "current_thread")]
    pub async fn run(&mut self) -> anyhow::Result<()> {
        let mut sc = signor::Client::new("localhost:12345").await?;

        let api = webrtc::api::APIBuilder::new().build();
        let (peer_conn, dc, side) = sc
            .connect(
                || async {
                    let peer_conn = api
                        .new_peer_connection(
                            webrtc::peer_connection::configuration::RTCConfiguration {
                                ..Default::default()
                            },
                        )
                        .await?;
                    let dc = peer_conn
                        .create_data_channel(
                            "tango",
                            Some(
                                webrtc::data_channel::data_channel_init::RTCDataChannelInit {
                                    id: Some(1),
                                    negotiated: Some(true),
                                    ordered: Some(true),
                                    ..Default::default()
                                },
                            ),
                        )
                        .await?;
                    Ok((peer_conn, dc))
                },
                &self.session_id,
            )
            .await?;
        let dc = datachannel::DataChannel::new(dc).await;

        // TODO: Other negotiation stuff.
        log::info!(
            "local sdp: {}",
            peer_conn.local_description().await.unwrap().sdp
        );
        log::info!(
            "remote sdp: {}",
            peer_conn.remote_description().await.unwrap().sdp
        );

        let mut nonce = [0u8; 16];
        rand::rngs::OsRng {}.fill(&mut nonce);
        let commitment = make_rng_commitment(&nonce)?;

        dc.send(
            protocol::Packet {
                which: Some(protocol::packet::Which::Hello(protocol::Hello {
                    protocol_version: protocol::VERSION,
                    game_title: self.game_title.clone(),
                    game_crc32: self.game_crc32,
                    match_type: self.match_type as u32,
                    rng_commitment: commitment.to_vec(),
                })),
            }
            .encode_to_vec()
            .as_slice(),
        )
        .await?;

        let hello = match protocol::Packet::decode(
            match dc.receive().await {
                Some(d) => d,
                None => anyhow::bail!("did not receive packet from peer"),
            }
            .as_slice(),
        )? {
            protocol::Packet {
                which: Some(protocol::packet::Which::Hello(hello)),
            } => hello,
            p => {
                anyhow::bail!("expected hello, got {:?}", p)
            }
        };

        if commitment.ct_eq(hello.rng_commitment.as_slice()).into() {
            anyhow::bail!("peer replayed our commitment")
        }

        if hello.protocol_version != protocol::VERSION {
            anyhow::bail!(
                "protocol version mismatch: {} != {}",
                hello.protocol_version,
                protocol::VERSION
            );
        }

        if hello.match_type != self.match_type as u32 {
            anyhow::bail!(
                "match type mismatch: {} != {}",
                hello.match_type,
                self.match_type
            );
        }

        if hello.game_title[..8] != self.game_title[..8] {
            anyhow::bail!("game mismatch: {} != {}", hello.game_title, self.game_title);
        }

        dc.send(
            protocol::Packet {
                which: Some(protocol::packet::Which::Hola(protocol::Hola {
                    rng_nonce: nonce.to_vec(),
                })),
            }
            .encode_to_vec()
            .as_slice(),
        )
        .await?;

        let hola = match protocol::Packet::decode(
            match dc.receive().await {
                Some(d) => d,
                None => anyhow::bail!("did not receive packet from peer"),
            }
            .as_slice(),
        )? {
            protocol::Packet {
                which: Some(protocol::packet::Which::Hola(hola)),
            } => hola,
            p => {
                anyhow::bail!("expected hello, got {:?}", p)
            }
        };

        if make_rng_commitment(&hola.rng_nonce)?
            .ct_eq(hello.rng_commitment.as_slice())
            .into()
        {
            anyhow::bail!("failed to verify rng commitment")
        }

        let seed = hola
            .rng_nonce
            .iter()
            .zip(nonce.iter())
            .map(|(&x1, &x2)| x1 ^ x2)
            .collect::<Vec<u8>>();

        let rng = rand_pcg::Mcg128Xsl64::from_seed(
            seed[..128]
                .iter()
                .zip(seed[128..].iter())
                .map(|(&x1, &x2)| x1 ^ x2)
                .collect::<Vec<u8>>()
                .try_into()
                .unwrap(),
        );

        *self.negotiation.lock() = Negotiation::Negotiated { dc, rng };

        Ok(())
    }

    pub fn poll_for_ready(&self) -> anyhow::Result<bool> {
        match &*self.negotiation.lock() {
            Negotiation::Negotiated { .. } => Ok(true),
            Negotiation::NotReady => Ok(false),
            Negotiation::Err(e) => Err(anyhow::anyhow!("{}", e)),
        }
    }
}

pub struct Battle {
    is_p2: bool,
    iq: input::Queue,
    local_pending_turn_wait_ticks_left: i32,
    local_pending_turn: Option<[u8; 0x100]>,
    remote_delay: u32,
    is_accepting_input: bool,
    is_over: bool,
    last_committed_remote_input: input::Input,
    last_input: Option<[input::Input; 2]>,
    state_committed: (), // TODO: what type should this be?
    committed_state: Option<mgba::state::State>,
}

impl Battle {
    pub fn local_player_index(&self) -> u8 {
        if self.is_p2 {
            1
        } else {
            0
        }
    }

    pub fn remote_player_index(&self) -> u8 {
        1 - self.local_player_index()
    }
}
