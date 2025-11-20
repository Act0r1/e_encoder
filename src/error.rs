
use thiserror::Error;
use tycho_simulation::tycho_core::simulation::errors::SimulationError;

#[derive(Error, Debug)]
pub enum StateErrors {
    #[error("Can't connect to the server")]
    Disconnect(#[from] SimulationError),
}
