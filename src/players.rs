use crate::traits::sync::{SyncPlayer, SyncPlayerList};
use crate::DynResult;

#[cfg(feature = "vlchttp")]
mod vlchttp;

#[cfg(feature = "vlcrc")]
mod vlcrc;

#[cfg(feature = "netflix")]
mod netflix_chrome;

#[cfg(all(feature = "mprisplayer", target_family = "unix"))]
mod mpris_player;

pub struct BulkSyncPlayerList {
    #[cfg(all(feature = "mprisplayer", target_family = "unix"))]
    mpris: mpris_player::MprisPlayerList,
    #[cfg(feature = "vlcrc")]
    vlcrc: vlcrc::VlcRcList,
    #[cfg(feature = "vlchttp")]
    vlchttp: vlchttp::VlcHttpList,
    #[cfg(feature = "netflix")]
    netflix_chrome: netflix_chrome::NetflixPlayerList,
}

impl SyncPlayerList for BulkSyncPlayerList {
    fn new() -> DynResult<Self> {
        #[cfg(all(feature = "mprisplayer", target_family = "unix"))]
        let mpris = mpris_player::MprisPlayerList::new()?;
        #[cfg(feature = "vlcrc")]
        let vlcrc = vlcrc::VlcRcList::new()?;
        #[cfg(feature = "vlchttp")]
        let vlchttp = vlchttp::VlcHttpList::new()?;
        #[cfg(feature = "netflix")]
        let netflix_chrome = netflix_chrome::NetflixPlayerList::new()?;
        Ok(Self {
            #[cfg(all(feature = "mprisplayer", target_family = "unix"))]
            mpris,
            #[cfg(feature = "vlcrc")]
            vlcrc,
            #[cfg(feature = "vlchttp")]
            vlchttp,
            #[cfg(feature = "netflix")]
            netflix_chrome,
        })
    }
    fn get_players(&mut self) -> DynResult<Vec<(String, Box<dyn SyncPlayer>)>> {
        let mut retvl = Vec::new();

        #[cfg(all(feature = "mprisplayer", target_family = "unix"))]
        {
            let mut mpris_players = self.mpris.get_players()?;
            retvl.append(&mut mpris_players);
        }

        #[cfg(feature = "vlcrc")]
        {
            let mut vlcrc_players = self.vlcrc.get_players()?;
            retvl.append(&mut vlcrc_players);
        }

        #[cfg(feature = "vlchttp")]
        {
            let mut vlchttp_players = self.vlchttp.get_players()?;
            retvl.append(&mut vlchttp_players);
        }

        #[cfg(feature = "netflix")]
        {
            let mut netflix_chrome_players = self.netflix_chrome.get_players()?;
            retvl.append(&mut netflix_chrome_players);
        }

        Ok(retvl)
    }
}
