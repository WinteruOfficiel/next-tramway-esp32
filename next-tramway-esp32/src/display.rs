// This modules defines the data structures and traits related to the UI state and commands
// It also contains the logic to apply commands to the UI state, but does not contain any rendering logic

// the rendering logic is implemented in the lcd module, which implements the TramDisplay trait for the Lcd struct

#[derive(Debug)]
pub enum UiCommand {
    UpdateDirection {
        line: heapless::String<16>,
        direction_id: usize,
        next_passages: heapless::Vec<TramNextPassage, 3>, 
        update_at: heapless::String<10>
    },
    UpdateMessage(heapless::String<80>),
    NextScreen
}

// main data structure representing the current state of the UI, which can be rendered by a TramDisplay implementation
#[derive(Debug)]
pub struct UiState {
    pub lines: heapless::Vec<TramLineState, 8>, // next passages data
    pub current_message: Option<heapless::String<80>>, // Log message to display, it's up to the display implementation to decide when (and if) to show it (e.g. only when there are no lines to display)
    pub current_line: usize, // index of the currently displayed line in `lines`, used for cycling through lines when there are more lines than can be displayed at once
    pub current_direction_id: usize // id of the currently displayed direction for the current line  
}

// represents the state of a single tram line, which can have multiple directions (towards both directions of the line)
#[derive(Debug)]
pub struct TramLineState {
    pub line: heapless::String<16>, // Display name of the line, e.g. "Tram C"
    pub directions: heapless::Vec<TramDirectionState, 2>, // for now we assume that there are at most 2 directions per line, but this can be easily changed if needed
}

#[derive(Debug, Clone, PartialEq)]
pub struct TramDirectionState {
    pub update_at: heapless::String<10>, // timestamp of the last update, used to display the freshness of the data
    pub direction_id: usize, // id of the direction, uncoupled from the index in the `directions` vector (e.g: tramway in grenoble used 1 and 2 as direction_id) could be upgraded to a string if needed
    pub next_passages: heapless::Vec<TramNextPassage, 3>,  // list of the next passages for this direction, we assume that there are at most 3 passages to display
}

#[derive(Debug, Clone, PartialEq)]
pub struct TramNextPassage {
    pub destination: heapless::String<32>, // display name of the destination of the tram, e.g. "Gare"
    pub relative_arrival: u8 // relative arrival time in minutes, used to display the time until the next tram arrives
}

// trait that defines the interface for rendering the UI state, which can be implemented by different display types (e.g. LCD, OLED, etc.)
pub trait TramDisplay {
    fn render<'a>(&'a mut self, state: &'a UiState) -> impl core::future::Future<Output = ()> + 'a;
}

// When we receive a ui command, we need to update the UI state accordingly, this function contains the logic to do so
pub fn apply_ui_command(state: &mut UiState, cmd: UiCommand) {
    match cmd {
        UiCommand::UpdateDirection { line, direction_id, next_passages, update_at } => {
            if let Some(line_state) = state.lines.iter_mut().find(|l| l.line == line) {
                if let Some(dir_state) = line_state
                    .directions
                        .iter_mut()
                        .find(|d| d.direction_id == direction_id)
                {
                    // update the existing direction state with the new passages and update_at timestamp

                    // we assume the backend already sorted the passages by arrival time
                    dir_state.next_passages = next_passages;
                    dir_state.update_at = update_at;
                } else {
                    let _ = line_state.directions.push(
                        TramDirectionState {
                            update_at,
                            direction_id,
                            next_passages
                        }
                    );
                }
            } else {
                // if the line doesn't exist yet in the state, we create a new line state and add it to the list of lines
                let mut new_line = TramLineState {
                    line,
                    directions: heapless::Vec::new()
                };
                let _ = new_line.directions.push(
                    TramDirectionState {
                        update_at,
                        direction_id,
                        next_passages,
                }
                );

                let _ = state.lines.push(new_line);
            }
        },
        UiCommand::NextScreen => {
            // in the current implementation, this is controlled by a button, could also be a timer to automatically cycle through the screens

            let lines = &state.lines;
            if lines.is_empty() {
                return;
            }

            state.current_direction_id += 1;

            let line = &lines[state.current_line];
            if state.current_direction_id >= line.directions.len() {
                state.current_direction_id = 0;
                state.current_line = (state.current_line + 1) % lines.len();
            }
        },
        UiCommand::UpdateMessage(string_inner) => {
            state.current_message = Some(string_inner);
        }
    }
}
