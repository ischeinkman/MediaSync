mod mpris_player;
use super::DynResult;
use mpris_player::MprisPlayerList;

use super::traits::sync::{SyncPlayer, SyncPlayerList};

pub struct BulkSyncPlayerList {
    mpris: MprisPlayerList,
}

impl SyncPlayerList for BulkSyncPlayerList {
    fn new() -> DynResult<Self> {
        let mpris = MprisPlayerList::new()?;
        Ok(Self { mpris })
    }
    fn get_players(&mut self) -> DynResult<Vec<(String, Box<dyn SyncPlayer>)>> {
        let mut retvl = Vec::new();
        let mut mpris_players = self.mpris.get_players()?;
        retvl.append(&mut mpris_players);
        Ok(retvl)
    }
}
