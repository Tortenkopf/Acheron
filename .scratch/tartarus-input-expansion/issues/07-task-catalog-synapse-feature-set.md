Type: task

Status: resolved

## Question

Catalog Razer Synapse's remap/macro feature set for the Tartarus Pro, minus cloud sync and lighting (both already out of scope for Acheron) — a hands-on task, not a decision, since it requires the user to spin up Synapse on a separate Windows machine.

Produce notes covering at least:

- Every distinct output/Action kind Synapse offers for a bound Input (single key, key combo, macro, profile switch, launch-application, text-block/snippet, media-key, mouse-button — whichever of these actually exist in Synapse's UI for this device).
- Every Trigger-mode-equivalent concept Synapse exposes (on-press, on-release, repeat-while-held, toggle, or anything finer-grained Acheron doesn't have yet).
- Anything Synapse does with Profiles/Layers ("Hypershift") beyond what Acheron's model already covers.
- Anything else present in Synapse's UI for this device that isn't cloud sync or lighting, even if its purpose is unclear at a glance — better to over-capture than to silently miss something.

The output is raw notes (a file, screenshots, whatever's fastest to capture) linked from this ticket's answer — not a decision about what Acheron should adopt. That judgment call belongs to the ticket this one blocks.

## Answer

Output/Actions offered by Razer Synapse:

* Grid Keys
  * Controller (Analog)
    * Offers the same kind of features as an X-Box controller. (In fact Razer Synapse creates a full blown kernel level virtual controller for this in order to circumvent common anti-cheat tools. I do not think we need to go that far. Anti cheat is really only an issue in e-sport type titles and people who want to play those will likely have to stick to Windows anyway for the same reason Synapse needs to circumvent the anti-cheat in the first place. Namely that these titles enforde the use of said anti cheat tools.)
    * This touches on the not yet implemented or tested capability of receiving analog signals from the Tartarus Pro's grid buttons, but is not dependent on it. Acharon could still simulate a fixed % output on a simulated analog axis even if no analog input is actually present and in some cases/games that might be enough. But ideally this would use the analog input if we can get it to work.
  * Joystick (Analog)
    * This aims to emulate any generic analog joystick/game-controller and offers the following:
      * 24 bindable buttons
      * analog X,Y,Z axis output
      * Up Right (X+,Y-), Up Left (X-, Y-), Down Left (X-, Y+), Down Right (Y-, Y+) as digital output
      * Single step increments and detrements to all the three axis
  * Keyboard
    * All possible keyboard buttons as bindable actions. Even individual symbols normally only reachable by key combinations are listed here. I am unsure if this then outputs as the combination or directly as unicode
    * An additional feature here is to define a trigger point based on how far a grid key has been depressed. This is read from the grid key's analog signal. Again this feature would be dependent on us actually managing to implement reading the analog signals.
  * Mouse Functions
    Left, Right, Middle, 4, 5, scroll up down left right
  * Macro
    * Synapse has it's own seperate Macro editor and saves Macros as seperate entities that can then be assigned/switched to any physical key. This is a behavior that we should copy as it enables users to swap predefined macros from one key to another without having to rebuild the macro for the new key.
    * Synapse allows only Keyboard and Mouse Functions to be assigned to Macros, not Controller or Joystick input.
      * I think this may again be an anti anti-cheat measure on the side of Synapse. Beyong that I see no reason to not allow Macros to also fire Controller or Joystick buttons. I would not do axis inputs though. That is probably too complicated.
    * Beyond what we already have the Maco tool in Synapse offers: Shell commands ( am not sure if we need or want this one to be honet.), Canned Text input (I am unsure of how this differs from a shell command, but those probably get somehow explicitly rooted to text shell.),A loop command
  * Switch to a different Profile
    * This feature is already planned on the current map.
  * Launch a Program (In my opinion this one is pretty useless and we do not need it.)
  * Multimedia
    * Offere Multimedia Controlls like Vol+, Vol-, Mute, Mic+, Mic-, MicMute, Mute All, Play/Pause, Previous Title, Next Title.
    * I have no idea why anyone would want to bind these to a game controller as there are much more useful things you could do with it, but maybe some people would want to bind volume or title controls to the Tartarus' Mouse Wheel. Lets make this one a maybe.
* Mouse Wheel
  * Razer Synapses utilization of the Mouse Wheel is pretty underwhelming as it again cannot fire controller or joystick buttons (see above for my speculation as to why.) and can only fire Keyboard Keys, Macros, Mouse Buttons and Multimedia Controlls.
  * Ergo: We are already much cooler in this regard and as soon as we get the Stepper feature working we will have surpassed Synapse in this area.
* Thumbstick
  * Razer Synapse can optionaly simulate four virtual bindings for the diagonial thumbstick directions. Those are purely virtual and are just the combination of the two adjacent cardinal directions as the stick has only 4 switches. The Thumbstick is NOT analog.
  * Synapse allows the binding of Keyboard Key, Mouse Buttons and Analog Joystick buttons and axis to these thumbstick directions, but not X-Box controller axis or buttons. Again I see no readon to adhere to this.
