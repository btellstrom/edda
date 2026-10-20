@pending
Feature: Who may do what

  Background:
    Given a 2024 campaign with a DM device, and player devices playing "Vex" and "Grog"

  @justifies:SPEC601
  Scenario: PE1 A player changes only their own character
    When the player playing Vex runs "edda grog set hp -5"
    Then the exit code is not 0
    And Grog's sheet is unchanged
    When the player playing Vex runs "edda grog"
    Then Grog's sheet is shown

  @justifies:SPEC604 @justifies:SPEC923
  Scenario: PE2 A player damages only their own combatants
    Given a started combat with Vex, Grog and Goblin 1
    When the player playing Vex enters "2 damage 3 5"
    Then the fight screen says before sending that only Grog's player or the DM may do that
    And nothing is sent

  @justifies:SPEC608
  Scenario: PE3 A player's concentration may name anyone
    Given a started combat with Vex, Grog and Goblin 1, on Vex's turn
    When the player playing Vex enters "cast hunter's mark 3"
    Then "Hunter's Mark" is on Goblin 1

  @justifies:SPEC604 @justifies:SPEC607
  Scenario: PE4 The services refuse what the screen would have caught
    Given a started combat with Vex and Goblin 1
    When the player playing Vex sends "end" to the combat service directly
    Then it is refused with 403

  @justifies:SPEC610 @justifies:SPEC611 @justifies:SPEC612
  Scenario Outline: PE5 Players see how hurt a monster looks, not its numbers
    Given a started combat with Vex and Goblin 1 of 10 hit points
    When Goblin 1 is at <hp> hit points
    Then the player playing Vex sees Goblin 1 as "<looks>"
    And nothing the player's device was sent holds Goblin 1's hit points or armor class

    Examples:
      | hp | looks    |
      | 10 | unhurt   |
      | 6  | hurt     |
      | 5  | bloodied |
      | 0  | down     |

  @justifies:SPEC614
  Scenario: PE6 Only the DM adds monsters
    When the player playing Vex runs "edda monster add troll.json"
    Then the exit code is not 0
    And the campaign has no monster "Cave Troll"

  @justifies:SPEC615 @justifies:SPEC616
  Scenario: PE7 Another campaign's things don't exist
    Given another campaign with a character "Pike"
    When the player playing Vex asks for Pike's sheet by its id
    Then it is not found
