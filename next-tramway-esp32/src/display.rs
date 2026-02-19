
#[derive(Debug)]
pub enum UiCommand {
    UpdateDirection {
        line: heapless::String<16>,
        direction_id: usize,
        next_passages: heapless::Vec<TramNextPassage, 3>, 
    }
}

#[derive(Debug)]
pub struct UiState {
    pub lines: heapless::Vec<TramLineState, 8>
}

#[derive(Debug)]
pub struct TramLineState {
    pub line: heapless::String<16>,
    pub directions: heapless::Vec<TramDirectionState, 2>,
}

#[derive(Debug)]
pub struct TramDirectionState {
    pub direction_id: usize,
    pub next_passages: heapless::Vec<TramNextPassage, 3>, 
}

#[derive(Debug)]
pub struct TramNextPassage {
    pub destination: heapless::String<32>,
    pub relative_arrival: u8
}

pub trait TramDisplay {
    fn render<'a>(&'a mut self, state: &'a UiState) -> impl core::future::Future<Output = ()> + 'a;
}

pub fn apply_ui_command(state: &mut UiState, cmd: UiCommand) {
    match cmd {
        UiCommand::UpdateDirection { line, direction_id, next_passages } => {
            if let Some(line_state) = state.lines.iter_mut().find(|l| l.line == line) {
                if let Some(dir_state) = line_state
                    .directions
                        .iter_mut()
                        .find(|d| d.direction_id == direction_id)
                {
                    dir_state.next_passages = next_passages;
                } else {
                    let _ = line_state.directions.push(
                        TramDirectionState {
                            direction_id,
                            next_passages
                        }
                    );
                }
            } else {
                let mut new_line = TramLineState {
                    line,
                    directions: heapless::Vec::new()
                };
                let _ = new_line.directions.push(
                    TramDirectionState {
                        direction_id,
                        next_passages,
                }
                );

                let _ = state.lines.push(new_line);
            }
        },
    }
}
