use async_tungstenite::tungstenite::Message as TMessage;
use base64::decode;
use futures::{ future, Stream, SinkExt, StreamExt };
use async_tungstenite::async_std::connect_async;
use protobuf::Message;
use serde::Serialize;
use std::sync::{ Arc, Mutex };
use tokio::sync::mpsc;

use crate::{ TradingSession };
use crate::yahoo::{ PricingData, PricingData_MarketHoursType };

use super::{ Quote };

#[derive(Debug, Clone, Serialize)]
struct Subs {
   subscribe: Vec<String>,
}

fn convert_session(value: PricingData_MarketHoursType) -> TradingSession {
   match value {
      PricingData_MarketHoursType::PRE_MARKET => TradingSession::PreMarket,
      PricingData_MarketHoursType::REGULAR_MARKET => TradingSession::Regular,
      PricingData_MarketHoursType::POST_MARKET => TradingSession::AfterHours,
      _ => TradingSession::Other,
   }
}

/// Realtime price quote streamer
///
/// To use it:
/// 1. Create a new streamer with `Streamer::new().await;`
/// 1. Subscribe to some symbols with `streamer.subscribe(vec!["AAPL"], |quote| /* do something */).await;`
/// 1. Let the streamer run `streamer.run().await;`
pub struct Streamer {
   subs: Vec<String>,
   shutdown: Arc<Mutex<bool>>
}
impl Streamer {
   pub fn new(symbols: Vec<&str>) -> Streamer {
      let mut subs = Vec::new();
      for symbol in &symbols { subs.push(symbol.to_string()); }

      Streamer { subs, shutdown: Arc::new(Mutex::new(false)) }
   }

   pub async fn stream(&self) -> impl Stream<Item = Quote> {
      let (tx, mut rx) = mpsc::unbounded_channel();

      let (stream, _) = connect_async("wss://streamer.finance.yahoo.com").await.unwrap();
      let (mut sink, source) = stream.split();

      // send the symbols we are interested in streaming
      let message = serde_json::to_string(&Subs { subscribe: self.subs.clone() }).unwrap();
      tx.send(TMessage::Text(message)).unwrap();

      // spawn a separate thread for sending out messages
      let shutdown = self.shutdown.clone();
      tokio::spawn(async move {
         loop {
            // stop on shutdown notification
            if *(shutdown.lock().unwrap()) { break; }

            // we're still running - so get a message and send it out.
            // TODO - change this to WAIT on receive so that we don't block shutdown
            if let Some(msg) = rx.recv().await {
               sink.send(msg).await.unwrap();
            } else {
               break;
            }
         }
      });

      let pong_tx = tx.clone();
      let shutdown = self.shutdown.clone();
      source
         .filter_map(move |msg| {
            match msg.unwrap() {
               TMessage::Ping(_) => { pong_tx.send(TMessage::Pong("pong".as_bytes().to_vec())).unwrap(); },
               TMessage::Close(_) => { *(shutdown.lock().unwrap()) = true; },
               TMessage::Text(value) => { return future::ready(Some(value)); },
               TMessage::Binary(value) => { return future::ready(Some(String::from_utf8(value).unwrap())); },
               _ => {}
            };
            return future::ready(None)
         })
         .map(move |msg| {
            let data = PricingData::parse_from_bytes(&decode(msg).unwrap()).unwrap();

            Quote {
               symbol: data.id.to_string(),
               timestamp: data.time as i64,
               session: convert_session(data.marketHours),
               price: data.price as f64,
               volume: data.dayVolume as u64
            }
         })
   }

   pub fn stop(&mut self) {
      let mut shutdown = self.shutdown.lock().unwrap();
      *shutdown = true;
   }
}