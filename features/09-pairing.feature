@pending
Feature: Pair a device with a spoken code

  Background:
    Given a campaign with a DM device "DM laptop"

  @justifies:SPEC728
  Scenario: PA1 A code read out pairs a device once the DM approves
    Given the DM runs "edda auth invite" and it shows a code like "7-tavern-dragon"
    When a new device runs "edda auth pair <CODE> --name Vex's laptop"
    And the DM approves "Vex's laptop"
    Then the new device is in the campaign as a player
    And "edda auth devices" lists "Vex's laptop"

  @justifies:SPEC729
  Scenario: PA2 The DM can decline
    Given the DM runs "edda auth invite"
    When a new device runs "edda auth pair <CODE> --name Stranger"
    And the DM declines
    Then the new device is not in the campaign

  @justifies:SPEC732
  Scenario: PA3 A wrong guess burns the code
    Given the DM runs "edda auth invite" and it shows "7-tavern-dragon"
    When a device runs "edda auth pair 7-tavern-goblin --name Guesser"
    Then the pairing fails
    And a device then pairing with "7-tavern-dragon" is told the code is no longer valid

  @justifies:SPEC731
  Scenario: PA4 A code admits only one device
    Given a device has paired with the DM's code
    When another device pairs with the same code
    Then the pairing fails

  @justifies:SPEC725
  Scenario: PA5 Codes are read forgivingly
    Given the DM's code is "7-tavern-dragon"
    When a new device runs "edda auth pair '7 Tavern DRAGON' --name Vex's laptop"
    Then the pairing goes ahead

  @justifies:SPEC710
  Scenario: PA6 A paired device trusts the auth service the DM vouched for
    Given a new device that reaches a different auth service than the DM's
    When it pairs with the DM's code
    Then it refuses the server, which isn't the one the DM's device knows
