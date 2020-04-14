mod mpris_player;
mod vlcrc;
mod vlchttp;
use super::DynResult;
use mpris_player::MprisPlayerList;

use super::traits::sync::{SyncPlayer, SyncPlayerList};

pub struct BulkSyncPlayerList {
    mpris: MprisPlayerList,
    vlcrc : vlcrc::VlcRcList, 
    vlchttp : vlchttp::VlcHttpList,
}

impl SyncPlayerList for BulkSyncPlayerList {
    fn new() -> DynResult<Self> {
        let mpris = MprisPlayerList::new()?;
        let vlcrc = vlcrc::VlcRcList::new()?;
        let vlchttp = vlchttp::VlcHttpList::new()?;
        Ok(Self { mpris, vlcrc, vlchttp })
    }
    fn get_players(&mut self) -> DynResult<Vec<(String, Box<dyn SyncPlayer>)>> {
        let mut retvl = Vec::new();

        let mut mpris_players = self.mpris.get_players()?;
        retvl.append(&mut mpris_players);

        let mut vlcrc_players = self.vlcrc.get_players()?;
        retvl.append(&mut vlcrc_players);
        
        let mut vlchttp_players = self.vlchttp.get_players()?;
        retvl.append(&mut vlchttp_players);

        Ok(retvl)
    }
}
