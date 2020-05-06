mod vlchttp;
mod vlcrc;
mod netflix_chrome;
use super::DynResult;

#[cfg(all(feature = "mprisplayer", target_family = "unix"))]
mod mpris_player;

#[cfg(not(all(feature = "mprisplayer", target_family = "unix")))]
mod mpris_player {
    use super::{DynResult, SyncPlayer, SyncPlayerList};
    pub struct MprisPlayerList {}
    impl SyncPlayerList for MprisPlayerList {
        fn new() -> DynResult<Self> {
            Ok(Self {})
        }
        fn get_players(&mut self) -> DynResult<Vec<(String, Box<dyn SyncPlayer>)>> {
            Ok(vec![])
        }
    }
}
use mpris_player::MprisPlayerList;

use super::traits::sync::{SyncPlayer, SyncPlayerList};

pub struct BulkSyncPlayerList {
    mpris: MprisPlayerList,
    vlcrc: vlcrc::VlcRcList,
    vlchttp: vlchttp::VlcHttpList,
    netflix_chrome : netflix_chrome::NetflixPlayerList, 
}

impl SyncPlayerList for BulkSyncPlayerList {
    fn new() -> DynResult<Self> {
        let mpris = MprisPlayerList::new()?;
        let vlcrc = vlcrc::VlcRcList::new()?;
        let vlchttp = vlchttp::VlcHttpList::new()?;
        let netflix_chrome = netflix_chrome::NetflixPlayerList::new()?;
        Ok(Self {
            mpris,
            vlcrc,
            vlchttp,
            netflix_chrome,
        })
    }
    fn get_players(&mut self) -> DynResult<Vec<(String, Box<dyn SyncPlayer>)>> {
        let mut retvl = Vec::new();

        let mut mpris_players = self.mpris.get_players()?;
        retvl.append(&mut mpris_players);

        let mut vlcrc_players = self.vlcrc.get_players()?;
        retvl.append(&mut vlcrc_players);

        let mut vlchttp_players = self.vlchttp.get_players()?;
        retvl.append(&mut vlchttp_players);

        let mut netflix_chrome_players = self.netflix_chrome.get_players()?;
        retvl.append(&mut netflix_chrome_players);

        Ok(retvl)
    }
}
