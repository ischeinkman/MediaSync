mod mprisplayer;
mod debug;

use std::collections::HashMap;
use crate::MyResult;
use crate::traits::{MediaPlayer, MediaPlayerList};

pub fn all_player_lists() -> MyResult<HashMap<String, Vec<String>>> {
    let mut retvl = HashMap::new();
    let mut mpris_player_list = mprisplayer::MprisPlayerList::new();
    let mpris_players = mpris_player_list.available()?;
    retvl.insert(mpris_player_list.list_name().to_owned(), mpris_players);
    
    let mut debug_player_list = debug::DebugPlayerList::new();
    let debug_players = debug_player_list.available()?;
    retvl.insert(debug_player_list.list_name().to_owned(), debug_players);

    Ok(retvl)
}

pub fn select_player(list : &str, player : &str) -> MyResult<Box<dyn MediaPlayer>> {
    let mut mpris_list = mprisplayer::MprisPlayerList::new();
    let mut debug_list = debug::DebugPlayerList::new();
    if list == mpris_list.list_name() {
        let player = mpris_list.open(player)?;
        Ok(Box::new(player))
    }
    else if list == debug_list.list_name() {
        let player = debug_list.open(player)?;
        Ok(Box::new(player))
    }
    else {
        Err(format!("Error: got invalid player identifier {}/{}", list, player).into())
    }
}