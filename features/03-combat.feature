@pending
Feature: Run a combat live

  Background:
    Given a 2024 campaign with a DM device and a player device playing "Vex"
    And the SRD is loaded
    And the DM has opened "edda fight --new Ambush"

  @justifies:SPEC205 @justifies:SPEC207 @justifies:SPEC213 @justifies:SPEC214 @justifies:SPEC215
  Scenario: CO1 Turns go round in initiative order
    Given the DM has entered "add vex", "add 2 goblin" and "roll"
    And the initiatives are Vex 15, Goblin 1 12 and Goblin 2 12
    When the DM enters "start"
    # The goblins tie on initiative and modifier, so the one who joined first goes first.
    Then the order is Vex, Goblin 1, Goblin 2
    When the DM enters "next" three times
    Then it is round 2 and Vex's turn

  @justifies:SPEC221 @justifies:SPEC222
  Scenario: CO2 Damage takes temporary hit points first
    Given Vex is in the fight with 30 of 40 hit points and 5 temporary
    When the DM deals 8 damage to Vex
    Then Vex has 27 hit points and 0 temporary
    When the DM heals Vex for 50
    Then Vex has 40 hit points

  @justifies:SPEC224 @justifies:SPEC225
  Scenario: CO3 A new concentration spell ends the old one
    Given Vex is concentrating on "Hunter's Mark" on Goblin 1
    When Vex casts "Bless" on herself, concentrating
    Then "Hunter's Mark" is no longer on Goblin 1
    And "Bless" is on Vex

  @justifies:SPEC226
  Scenario: CO4 Dropping to 0 hit points breaks concentration
    Given Vex is concentrating on "Hunter's Mark" on Goblin 1
    When the DM deals damage to Vex down to 0 hit points
    Then "Hunter's Mark" is no longer on Goblin 1

  @justifies:SPEC229
  Scenario: CO5 An effect lasts rounds of whoever put it down
    Given the combat has started and it is Vex's turn
    When Vex enters "effect Prone 2 for 1"
    And everyone takes a turn
    Then at the start of Vex's next turn Prone has ended

  @justifies:SPEC237 @justifies:SPEC239
  Scenario: CO6 Every device sees each change
    Given the player has the same fight open on another combat-service instance
    When the DM deals 7 damage to Goblin 1
    Then the player's screen shows it in the log at once

  @justifies:SPEC245
  Scenario: CO7 The screen keeps working through a dropped connection
    Given the player's connection drops
    When the player enters "damage 2 5"
    Then the player's screen shows Goblin 1 hurt at once
    When the connection comes back
    Then the command reaches the server once
    And every device shows the same log

  @justifies:SPEC233 @justifies:SPEC234
  Scenario: CO8 A paused fight resumes days later where it stopped
    Given the combat is in round 3 on Goblin 1's turn
    When the DM enters "pause" and quits
    And all combat-service instances restart
    And the DM runs "edda fight ambush" and enters "resume"
    Then it is round 3 and Goblin 1's turn

  @justifies:SPEC247 @justifies:SPEC251
  Scenario: CO9 Hit points and slots go back to the sheets
    Given Vex is in the fight with 2 2nd-level slots left on her sheet
    When the DM deals 7 damage to Vex
    And Vex enters "cast cure wounds 1 at 2"
    Then Vex's sheet shows her hit points after the damage and the healing
    And Vex's sheet shows 1 2nd-level slot left

  @justifies:SPEC252 @justifies:SPEC253
  Scenario: CO10 A refused cast costs nothing
    Given Vex has no 2nd-level slots left on her sheet
    When Vex enters "cast cure wounds 1 at 2"
    Then the cast is refused with the sheet's reason
    And nothing is added to the log

  @justifies:SPEC257
  Scenario: CO11 Monsters are numbered on from those already fighting
    Given Goblin 1 and Goblin 2 are in the fight
    When the DM enters "add 2 goblin"
    Then Goblin 3 and Goblin 4 join at their maximum hit points

  @justifies:SPEC262
  Scenario: CO12 A fight from a plan with a monster gone isn't started
    Given a plan "Cave" with 2 "Cave Troll" from the campaign's own monsters
    And the campaign's "Cave Troll" has been deleted
    When the DM runs "edda fight --plan cave"
    Then no combat is started
    And the error names "Cave Troll"

  @justifies:SPEC263
  Scenario: CO13 The log names the device, not who the client says it is
    When the player's device sends a command claiming to be "DM laptop"
    Then the log names the player's device
